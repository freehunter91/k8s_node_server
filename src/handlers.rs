
use crate::db::fetch_cluster_data;
use crate::models::ClusterInfo;
use crate::dummy_data::generate_dummy_clusters;
use actix::{Actor, Addr, Context, Handler, Message, StreamHandler, ActorContext, AsyncContext};
use actix_web::{web, Error, HttpRequest, HttpResponse};
use actix_web_actors::ws;
use kube::Client;
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use log::{error, info};

#[derive(Message)]
#[rtype(result = "()")]
struct Connect { pub addr: Addr<WsSession> }

#[derive(Message)]
#[rtype(result = "()")]
struct Disconnect { pub addr: Addr<WsSession> }

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
}

impl WsServer {
    pub fn new(clients: HashMap<String, Client>) -> Self {
        Self { 
            sessions: Vec::new(), 
            user_clients: Arc::new(clients),
            dummy_mode: false,
        }
    }

    pub fn new_with_dummy_data() -> Self {
        Self { 
            sessions: Vec::new(), 
            user_clients: Arc::new(HashMap::new()),
            dummy_mode: true,
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
                if !sessions.is_empty() {
                    tokio::spawn(async move {
                        for (name, client) in clients.iter() {
                            if let Some(info) = fetch_cluster_data(name.clone(), client.clone()).await {
                                for session_addr in &sessions {
                                    session_addr.do_send(info.clone());
                                }
                            }
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

pub async fn ws_index(req: HttpRequest, stream: web::Payload, srv: web::Data<Addr<WsServer>>) -> Result<HttpResponse, Error> {
    ws::start(WsSession { server_addr: srv.get_ref().clone() }, &req, stream)
}
