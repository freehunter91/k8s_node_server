use actix::{Actor, Addr, Context, Handler, Message, StreamHandler, ActorContext, AsyncContext};
use actix_files as fs;
use actix_web::{web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
use futures::future::join_all;
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::{
    api::{ListParams, ResourceExt},
    Client, Config,
    config::{KubeConfigOptions, Kubeconfig},
};
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use log::{error, info, warn};

// --- 데이터 모델 ---
// [수정] 액터 메시지로 사용될 구조체에 rtype을 명시합니다.
#[derive(Serialize, Deserialize, Clone, Debug, Message)]
#[rtype(result = "()")]
pub struct ClusterInfo {
    pub name: String,
    pub node_count: usize,
    pub pod_count: usize,
    pub nodes: Vec<NodeInfo>,
}

// [수정] 이 구조체들은 메시지가 아니므로 derive(Message)를 제거합니다.
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

// --- 액터 메시지 정의 ---
#[derive(Message)]
#[rtype(result = "()")]
struct Connect { pub addr: Addr<WsSession> }

#[derive(Message)]
#[rtype(result = "()")]
struct Disconnect { pub addr: Addr<WsSession> }

// --- 웹소켓 세션 액터 ---
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
        ctx.text(serde_json::to_string(&msg).unwrap());
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsSession {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        if let Err(e) = msg {
            error!("웹소켓 오류 발생: {}", e);
            ctx.stop();
        }
    }
}

// --- 중앙 관리 액터 ---
pub struct WsServer {
    sessions: Vec<Addr<WsSession>>,
    user_clients: Arc<HashMap<String, Client>>,
}

impl WsServer {
    pub fn new(clients: HashMap<String, Client>) -> Self {
        Self {
            sessions: Vec::new(),
            user_clients: Arc::new(clients),
        }
    }
}

impl Actor for WsServer {
    type Context = Context<Self>;

    // [최적화] 서버 액터가 시작될 때, 중앙에서 주기적으로 데이터 조회를 시작합니다.
    fn started(&mut self, ctx: &mut Self::Context) {
        info!("중앙 관리 서버 시작. 5초마다 데이터 폴링을 시작합니다.");
        ctx.run_interval(std::time::Duration::from_secs(5), |act, _ctx| {
            let clients = act.user_clients.clone();
            let sessions = act.sessions.clone();

            tokio::spawn(async move {
                for (name, client) in clients.iter() {
                    if let Some(info) = fetch_cluster_data(name.clone(), client.clone()).await {
                        for session_addr in &sessions {
                            session_addr.do_send(info.clone());
                        }
                    }
                }
            });
        });
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

// --- 데이터 조회 로직 ---
async fn fetch_cluster_data(name: String, client: Client) -> Option<ClusterInfo> {
    let nodes_api: kube::Api<Node> = kube::Api::all(client.clone());
    let pods_api: kube::Api<Pod> = kube::Api::all(client);

    let (nodes_res, pods_res) = tokio::join!(
        nodes_api.list(&ListParams::default()),
        pods_api.list(&ListParams::default())
    );

    match (nodes_res, pods_res) {
        (Ok(nodes), Ok(pods)) => {
            let mut pods_by_node: HashMap<String, Vec<Pod>> = HashMap::new();
            for pod in pods.items {
                if let Some(node_name) = &pod.spec.as_ref().and_then(|s| s.node_name.clone()) {
                    pods_by_node.entry(node_name.clone()).or_default().push(pod);
                }
            }
            let total_pod_count = pods_by_node.values().map(|v| v.len()).sum();
            let nodes_info: Vec<NodeInfo> = nodes.items.iter().map(|node| {
                let node_name = node.name_any();
                let node_pods_vec = pods_by_node.get(&node_name).cloned().unwrap_or_default();
                let mut container_count_for_node = 0;
                let node_pods_info: Vec<PodInfo> = node_pods_vec.iter().map(|p| {
                    let containers: Vec<ContainerInfo> = p.spec.as_ref().map(|s| s.containers.iter().map(|c| ContainerInfo {
                        name: c.name.clone(),
                        image: c.image.clone().unwrap_or_default(),
                    }).collect()).unwrap_or_default();
                    container_count_for_node += containers.len();
                    PodInfo { name: p.name_any(), namespace: p.namespace().unwrap_or_default(), node_name: node_name.clone(), labels: p.labels().clone(), containers, cluster_name: name.clone() }
                }).collect();
                let node_status = node.status.as_ref();
                let node_info_details = node_status.and_then(|s| s.node_info.as_ref());
                NodeInfo {
                    name: node_name.clone(), labels: node.labels().clone(), pods: node_pods_info, pod_count: node_pods_vec.len(), container_count: container_count_for_node, cluster_name: name.clone(),
                    os_image: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.os_image.clone()),
                    kubelet_version: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.kubelet_version.clone()),
                    architecture: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.architecture.clone()),
                    capacity_cpu: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("cpu").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()),
                    capacity_memory: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("memory").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()),
                }
            }).collect();
            Some(ClusterInfo { name: name.clone(), node_count: nodes.items.len(), pod_count: total_pod_count, nodes: nodes_info })
        }
        (Err(e), _) => { error!("[{}] 노드 조회 실패: {}", name, e); None },
        (_, Err(e)) => { error!("[{}] 파드 조회 실패: {}", name, e); None },
    }
}

// --- 웹소켓 연결 핸들러 ---
async fn ws_index(
    req: HttpRequest,
    stream: web::Payload,
    srv: web::Data<Addr<WsServer>>,
) -> Result<HttpResponse, Error> {
    ws::start(WsSession { server_addr: srv.get_ref().clone() }, &req, stream)
}

// --- 메인 함수 ---
#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    let kubeconfig = match Kubeconfig::read() {
        Ok(conf) => conf,
        Err(e) => {
            error!("Kubeconfig 파일을 읽는 데 실패했습니다: {}. 서버를 시작할 수 없습니다.", e);
            return Ok(());
        }
    };

    // [최적화] 모든 클라이언트 생성을 비동기적으로 동시에 기다린 후 서버를 시작합니다.
    let mut client_creation_futures = vec![];
    for context in &kubeconfig.contexts {
        let context_name = context.name.clone();
        let kubeconfig_clone = kubeconfig.clone();
        
        client_creation_futures.push(tokio::spawn(async move {
            info!("[{}] 클라이언트 생성을 시도합니다...", context_name);
            let options = KubeConfigOptions { context: Some(context_name.clone()), ..Default::default() };
            let mut client_config = Config::from_custom_kubeconfig(kubeconfig_clone, &options).await?;
            
            // 중요: 이 부분에 실제 Keycloak 토큰을 넣거나, 토큰 갱신 로직을 추가해야 합니다.
            client_config.auth_info.token = Some("여기에_ID_토큰을_붙여넣으세요".into());
            client_config.auth_info.exec = None;

            let client = Client::try_from(client_config)?;
            info!("[{}] 클라이언트 생성 성공", context_name);
            Ok((context_name, client)) as Result<(String, Client), kube::Error>
        }));
    }

    let results = join_all(client_creation_futures).await;
    let mut user_clients = HashMap::new();
    for result in results {
        match result {
            Ok(Ok((name, client))) => {
                user_clients.insert(name, client);
            }
            Ok(Err(e)) => error!("클라이언트 생성 실패: {}", e),
            Err(e) => error!("클라이언트 생성 작업 실패: {}", e),
        }
    }

    if user_clients.is_empty() {
        warn!("성공적으로 생성된 클라이언트가 없습니다.");
    }
    
    let server = WsServer::new(user_clients);
    let server_addr = server.start();

    info!("웹 서버를 http://127.0.0.1:8080 에서 시작합니다.");

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

