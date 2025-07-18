
use crate::db::fetch_cluster_data;
use crate::models::ClusterInfo;
use crate::dummy_data::generate_dummy_clusters;
use crate::excel::generate_node_excel;
use actix::{Actor, Addr, Context, Handler, Message, StreamHandler, ActorContext, AsyncContext};
use actix_web::{web, Error, HttpRequest, HttpResponse};
use actix_web_actors::ws;
use kube::Client;
use serde_json;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use log::{error, info};

#[derive(Message)]
#[rtype(result = "()")]
struct Connect { pub addr: Addr<WsSession> }

#[derive(Message)]
#[rtype(result = "()")]
struct Disconnect { pub addr: Addr<WsSession> }

#[derive(Message)]
#[rtype(result = "Vec<ClusterInfo>")]
struct GetCurrentClusters;

pub struct WsSession {
    pub server_addr: Addr<WsServer>,
}

impl Actor for WsSession {
    type Context = ws::WebsocketContext<Self>;
    fn started(&mut self, ctx: &mut Self::Context) {
        info!("새로운 웹소켓 세션 시작됨.");
        self.server_addr.do_send(Connect { addr: ctx.address() });
    }
    fn stopping(&mut self, ctx: &mut Self::Context) -> actix::Running {
        self.server_addr.do_send(Disconnect { addr: ctx.address() });
        actix::Running::Stop
    }
}

impl Handler<ClusterInfo> for WsSession {
    type Result = ();
    fn handle(&mut self, msg: ClusterInfo, ctx: &mut Self::Context) {
        if let Ok(json) = serde_json::to_string(&msg) {
            ctx.text(json);
        }
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsSession {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(msg)) => ctx.pong(&msg),
            Ok(ws::Message::Close(reason)) => { ctx.close(reason); ctx.stop(); },
            Err(e) => { error!("웹소켓 오류 발생: {}", e); ctx.stop(); },
            _ => {}
        }
    }
}

pub struct WsServer {
    sessions: Vec<Addr<WsSession>>,
    user_clients: Arc<HashMap<String, Client>>,
    dummy_mode: bool,
    current_clusters: Arc<RwLock<Vec<ClusterInfo>>>,
}

impl WsServer {
    pub fn new(clients: HashMap<String, Client>) -> Self {
        Self { 
            sessions: Vec::new(), 
            user_clients: Arc::new(clients),
            dummy_mode: false,
            current_clusters: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn new_with_dummy_data() -> Self {
        Self { 
            sessions: Vec::new(), 
            user_clients: Arc::new(HashMap::new()),
            dummy_mode: true,
            current_clusters: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Actor for WsServer {
    type Context = Context<Self>;
    fn started(&mut self, ctx: &mut Self::Context) {
        if self.dummy_mode {
            info!("더미 데이터 모드로 중앙 관리 서버 시작. 5초마다 더미 데이터를 전송합니다.");
            ctx.run_interval(std::time::Duration::from_secs(5), |act, _ctx| {
                let sessions = act.sessions.clone();
                if !sessions.is_empty() {
                    let dummy_clusters = generate_dummy_clusters();
                    // 현재 클러스터 데이터 업데이트
                    if let Ok(mut clusters) = act.current_clusters.write() {
                        *clusters = dummy_clusters.clone();
                    }
                    for cluster_info in dummy_clusters {
                        for session_addr in &sessions {
                            session_addr.do_send(cluster_info.clone());
                        }
                    }
                }
            });
        } else {
            info!("중앙 관리 서버 시작. 5초마다 데이터 폴링을 시작합니다.");
            ctx.run_interval(std::time::Duration::from_secs(5), |act, _ctx| {
                let clients = act.user_clients.clone();
                let sessions = act.sessions.clone();
                let current_clusters = act.current_clusters.clone();
                if !sessions.is_empty() {
                    tokio::spawn(async move {
                        let mut all_clusters = Vec::new();
                        for (name, client) in clients.iter() {
                            if let Some(info) = fetch_cluster_data(name.clone(), client.clone()).await {
                                all_clusters.push(info.clone());
                                for session_addr in &sessions {
                                    session_addr.do_send(info.clone());
                                }
                            }
                        }
                        // 현재 클러스터 데이터 업데이트
                        if let Ok(mut clusters) = current_clusters.write() {
                            *clusters = all_clusters;
                        }
                    });
                }
            });
        }
    }
}

impl Handler<Connect> for WsServer {
    type Result = ();
    fn handle(&mut self, msg: Connect, _: &mut Context<Self>) {
        info!("세션 연결됨.");
        self.sessions.push(msg.addr);
    }
}

impl Handler<Disconnect> for WsServer {
    type Result = ();
    fn handle(&mut self, msg: Disconnect, _: &mut Context<Self>) {
        info!("세션 연결 끊김.");
        self.sessions.retain(|addr| *addr != msg.addr);
    }
}

impl Handler<GetCurrentClusters> for WsServer {
    type Result = Vec<ClusterInfo>;
    
    fn handle(&mut self, _msg: GetCurrentClusters, _ctx: &mut Self::Context) -> Self::Result {
        if let Ok(clusters) = self.current_clusters.read() {
            clusters.clone()
        } else {
            // 실패한 경우 더미 데이터 반환
            if self.dummy_mode {
                generate_dummy_clusters()
            } else {
                Vec::new()
            }
        }
    }
}

pub async fn ws_index(req: HttpRequest, stream: web::Payload, srv: web::Data<Addr<WsServer>>) -> Result<HttpResponse, Error> {
    ws::start(WsSession { server_addr: srv.get_ref().clone() }, &req, stream)
}

pub async fn download_node_excel(
    path: web::Path<(String, String)>,
    srv: web::Data<Addr<WsServer>>,
) -> Result<HttpResponse, Error> {
    let (cluster_name, node_name) = path.into_inner();
    
    // WsServer에서 현재 데이터 가져오기
    let current_clusters = get_current_clusters_data(srv.get_ref()).await;
    
    // 요청된 클러스터와 노드 찾기
    if let Some(cluster) = current_clusters.iter().find(|c| c.name == cluster_name) {
        if let Some(node) = cluster.nodes.iter().find(|n| n.name == node_name) {
            return generate_excel_response(node, &cluster_name, &node_name);
        }
    }
    
    // 노드를 찾지 못한 경우
    Ok(HttpResponse::NotFound().json(serde_json::json!({
        "error": format!("Node '{}' not found in cluster '{}'", node_name, cluster_name)
    })))
}

async fn get_current_clusters_data(server_addr: &Addr<WsServer>) -> Vec<ClusterInfo> {
    // 서버에서 현재 클러스터 데이터 요청
    match server_addr.send(GetCurrentClusters).await {
        Ok(clusters) => clusters,
        Err(_) => {
            // 요청 실패 시 더미 데이터 반환
            generate_dummy_clusters()
        }
    }
}

fn generate_excel_response(node: &crate::models::NodeInfo, cluster_name: &str, node_name: &str) -> Result<HttpResponse, Error> {
    match generate_node_excel(node) {
        Ok(excel_data) => {
            let filename = format!("{}_{}_node_info.xlsx", cluster_name, node_name);
            Ok(HttpResponse::Ok()
                .content_type("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
                .insert_header(("Content-Disposition", format!("attachment; filename=\"{}\"", filename)))
                .body(excel_data))
        }
        Err(e) => {
            error!("Excel 파일 생성 실패: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Excel 파일 생성에 실패했습니다"
            })))
        }
    }
}
