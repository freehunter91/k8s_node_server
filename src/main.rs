use actix::{Actor, Addr, Context, Handler, Message, StreamHandler};
use actix_files as fs;
use actix_web::{web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::{
    api::{ListParams, ResourceExt},
    Client, Config,
    config::{KubeConfigOptions, Kubeconfig},
};
use rand::{thread_rng, Rng};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use log::{error, info, warn};

// 데이터 모델
#[derive(Serialize, Deserialize, Clone, Debug, Message)]
#[rtype(result = "()")]
pub struct ClusterInfo {
    pub name: String,
    pub node_count: usize,
    pub pod_count: usize,
    pub nodes: Vec<NodeInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NodeInfo {
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub pods: Vec<PodInfo>,
    pub pod_count: usize,
    pub container_count: usize,
    pub cluster_name: String,
    pub os_image: String,
    pub kubelet_version: String,
    pub architecture: String,
    pub capacity_cpu: String,
    pub capacity_memory: String,
    pub gpu_model: String,
    pub gpu_count: String,
    pub mig_devices: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PodInfo {
    pub name: String,
    pub namespace: String,
    pub node_name: String,
    pub labels: BTreeMap<String, String>,
    pub containers: Vec<ContainerInfo>,
    pub cluster_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
}

// WebSocket 메시지 정의
#[derive(Message)]
#[rtype(result = "()")]
struct Connect { pub addr: Addr<WsSession>, pub id: usize }

#[derive(Message)]
#[rtype(result = "()")]
struct Disconnect { pub id: usize }

pub struct WsSession {
    pub id: usize,
    pub server_addr: Addr<WsServer>,
}

impl Actor for WsSession {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("🌐 WebSocket 세션 시작됨 - ID: {}", self.id);
        self.server_addr.do_send(Connect { addr: ctx.address(), id: self.id });
    }

    fn stopping(&mut self, _: &mut Self::Context) -> actix::Running {
        info!("❌ WebSocket 세션 종료됨 - ID: {}", self.id);
        self.server_addr.do_send(Disconnect { id: self.id });
        actix::Running::Stop
    }
}

impl Handler<ClusterInfo> for WsSession {
    type Result = ();

    fn handle(&mut self, msg: ClusterInfo, ctx: &mut Self::Context) {
        match serde_json::to_string(&msg) {
            Ok(json) => {
                info!("📤 클러스터 '{}' 데이터 전송 - 세션 ID: {}", msg.name, self.id);
                ctx.text(json);
            }
            Err(e) => {
                error!("JSON 직렬화 실패: {}", e);
            }
        }
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsSession {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        if msg.is_err() {
            error!("⚠ WebSocket 메시지 오류 - 세션 ID: {}", self.id);
            ctx.stop();
        }
    }
}

// 중앙 WebSocket 서버
pub struct WsServer {
    sessions: HashMap<usize, Addr<WsSession>>,
    user_clients: Arc<HashMap<String, Client>>,
}

impl WsServer {
    pub fn new(clients: HashMap<String, Client>) -> Self {
        Self {
            sessions: HashMap::new(),
            user_clients: Arc::new(clients),
        }
    }
}

impl Actor for WsServer {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("🚀 WsServer 시작됨. 5초 간격으로 클러스터 상태 조회 중...");

        ctx.run_interval(std::time::Duration::from_secs(5), |act, _ctx| {
            let clients = act.user_clients.clone();
            let sessions = act.sessions.clone();

            actix::spawn(async move {
                for (name, client) in clients.iter() {
                    if let Some(info) = fetch_cluster_data(name.clone(), client.clone()).await {
                        info!("✅ 클러스터 '{}' 데이터 수집 완료 - 노드 수: {}, 파드 수: {}",
                              info.name, info.node_count, info.pod_count);
                        for session in sessions.values() {
                            session.do_send(info.clone());
                        }
                    } else {
                        warn!("⚠ '{}' 클러스터 정보 수집 실패", name);
                    }
                }
            });
        });
    }
}

impl Handler<Connect> for WsServer {
    type Result = ();
    fn handle(&mut self, msg: Connect, _: &mut Context<Self>) {
        info!("➕ 세션 연결됨 - ID: {}", msg.id);
        self.sessions.insert(msg.id, msg.addr);
    }
}

impl Handler<Disconnect> for WsServer {
    type Result = ();
    fn handle(&mut self, msg: Disconnect, _: &mut Context<Self>) {
        info!("➖ 세션 연결 해제됨 - ID: {}", msg.id);
        self.sessions.remove(&msg.id);
    }
}

// 클러스터 정보 수집
async fn fetch_cluster_data(name: String, client: Client) -> Option<ClusterInfo> {
    let nodes_api: kube::Api<Node> = kube::Api::all(client.clone());
    let pods_api: kube::Api<Pod> = kube::Api::all(client);

    let lp = ListParams::default();
    let nodes_res = nodes_api.list(&lp).await;
    let pods_res = pods_api.list(&lp).await;

    match (nodes_res, pods_res) {
        (Ok(nodes), Ok(pods)) => {
            let mut pods_by_node: HashMap<String, Vec<Pod>> = HashMap::new();
            for pod in pods.items {
                if let Some(node_name) = &pod.spec.as_ref().and_then(|s| s.node_name.clone()) {
                    pods_by_node.entry(node_name.clone()).or_default().push(pod);
                }
            }

            let total_pod_count = pods_by_node.values().map(|v| v.len()).sum();
            let nodes_info: Vec<NodeInfo> = nodes.items.into_iter().map(|node| {
                let node_name = node.name_any();
                let pod_items = pods_by_node.get(&node_name).cloned().unwrap_or_default();
                let mut container_count = 0;

                let pod_infos: Vec<PodInfo> = pod_items.iter().map(|p| {
                    let containers: Vec<ContainerInfo> = p.spec.as_ref().map_or(vec![], |s| {
                        s.containers.iter().map(|c| ContainerInfo {
                            name: c.name.clone(),
                            image: c.image.clone().unwrap_or_default(),
                        }).collect()
                    });
                    container_count += containers.len();
                    PodInfo {
                        name: p.name_any(),
                        namespace: p.namespace().unwrap_or_default(),
                        node_name: node_name.clone(),
                        labels: p.labels().clone(),
                        containers,
                        cluster_name: name.clone(),
                    }
                }).collect();

                let status = node.status.unwrap_or_default();
                let info = status.node_info.unwrap_or_default();
                let labels = node.labels().clone();
                let gpu_model = labels.get("nvidia.com/gpu.product").cloned().unwrap_or("N/A".into());
                let gpu_count = status.capacity.as_ref()
                    .and_then(|c| c.get("nvidia.com/gpu"))
                    .map(|q| q.0.clone()).unwrap_or("0".into());

                let mut mig_devices = HashMap::new();
                if let Some(capacity) = status.capacity.as_ref() {
                    for (k, v) in capacity {
                        if let Some(profile) = k.strip_prefix("nvidia.com/mig-") {
                            mig_devices.insert(profile.to_string(), v.0.clone());
                        }
                    }
                }

                NodeInfo {
                    name: node_name,
                    labels,
                    pods: pod_infos.clone(),
                    pod_count: pod_infos.len(),
                    container_count,
                    cluster_name: name.clone(),
                    os_image: info.os_image,
                    kubelet_version: info.kubelet_version,
                    architecture: info.architecture,
                    capacity_cpu: status.capacity.as_ref().and_then(|c| c.get("cpu")).map(|q| q.0.clone()).unwrap_or("N/A".into()),
                    capacity_memory: status.capacity.as_ref().and_then(|c| c.get("memory")).map(|q| q.0.clone()).unwrap_or("N/A".into()),
                    gpu_model,
                    gpu_count,
                    mig_devices,
                }
            }).collect();

            Some(ClusterInfo {
                name,
                node_count: nodes_info.len(),
                pod_count: total_pod_count,
                nodes: nodes_info,
            })
        }
        _ => None,
    }
}

// WebSocket 핸들러
async fn ws_index(req: HttpRequest, stream: web::Payload, srv: web::Data<Addr<WsServer>>) -> Result<HttpResponse, Error> {
    let id: usize = thread_rng().gen();
    info!("🌐 WebSocket 연결 요청 수신 - 세션 ID: {}", id);
    ws::start(WsSession { id, server_addr: srv.get_ref().clone() }, &req, stream)
}

// 앱 시작점
#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    let kubeconfig = match Kubeconfig::read() {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Kubeconfig read failed: {}", e);
            return Ok(());
        },
    };

    let mut clients = HashMap::new();
    for ctx in &kubeconfig.contexts {
        let name = ctx.name.clone();
        let options = KubeConfigOptions { context: Some(name.clone()), ..Default::default() };
        if let Ok(mut cfg) = Config::from_custom_kubeconfig(kubeconfig.clone(), &options).await {
            cfg.auth_info.token = Some("your_id_token".into());
            cfg.auth_info.exec = None;
            if let Ok(client) = Client::try_from(cfg) {
                if kube::Api::<Pod>::all(client.clone()).list(&ListParams::default().limit(1)).await.is_ok() {
                    info!("✅ [{}] 클러스터 연결 성공", name);
                    clients.insert(name.clone(), client);
                }
            }
        }
    }

    let server = WsServer::new(clients);
    let server_addr = server.start();

    info!("🚀 웹서버 시작: http://127.0.0.1:8080");

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(server_addr.clone()))
            .route("/ws/", web::get().to(ws_index))
            .service(fs::Files::new("/", "./static").index_file("index.html"))
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}