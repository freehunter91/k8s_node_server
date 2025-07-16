use actix::{Actor, Addr, Context, Handler, Message, StreamHandler, ActorContext, AsyncContext};
use actix_files as fs;
use actix_web::{web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
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

// --- [수정] 데이터 모델: 프론트엔드에 필요한 모든 상세 정보를 포함하도록 복원 ---
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
    // [추가] GPU 관련 필드를 다시 추가합니다.
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


// --- 액터 메시지 정의 ---
#[derive(Message)]
#[rtype(result = "()")]
struct FetchData(pub Addr<WsSession>);


// --- 웹소켓 세션 액터 ---
pub struct WsSession {
    pub server_addr: Addr<WsServer>,
}

impl Actor for WsSession {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("새로운 웹소켓 세션 시작됨. 데이터 조회를 요청합니다.");
        self.server_addr.do_send(FetchData(ctx.address()));
    }
}

impl Handler<ClusterInfo> for WsSession {
    type Result = ();

    fn handle(&mut self, msg: ClusterInfo, ctx: &mut Self::Context) {
        info!("[{}] 클러스터 정보를 클라이언트로 전송합니다.", msg.name);
        ctx.text(serde_json::to_string(&msg).unwrap());
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsSession {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Text(text)) => {
                info!("클라이언트로부터 메시지 수신: {}", text);
                ctx.text(format!("Echo: {}", text));
            }
            Ok(ws::Message::Ping(msg)) => ctx.pong(&msg),
            Ok(ws::Message::Close(reason)) => ctx.close(reason),
            Err(e) => {
                error!("웹소켓 오류 발생: {}", e);
                ctx.stop();
            }
            _ => {}
        }
    }
}

// --- 중앙 관리 액터 ---
pub struct WsServer {
    pub user_clients: Arc<Mutex<HashMap<String, Client>>>,
}

// [수정] FetchData 메시지를 받았을 때의 처리 로직
impl Handler<FetchData> for WsServer {
    type Result = ();

    fn handle(&mut self, msg: FetchData, _ctx: &mut Self::Context) {
        info!("데이터 조회 요청 수신. 클러스터 정보 수집을 시작합니다.");
        let clients = self.user_clients.lock().unwrap().clone();
        let session_addr = msg.0;

        for (name, client) in clients {
            let session_addr_clone = session_addr.clone();
            tokio::spawn(async move {
                info!("[{}] 노드 및 파드 정보 조회 중...", name);
                let nodes_api: kube::Api<Node> = kube::Api::all(client.clone());
                let pods_api: kube::Api<Pod> = kube::Api::all(client);

                let lp = ListParams::default();
                let nodes_res = nodes_api.list(&lp).await;
                let pods_res = pods_api.list(&lp).await;

                match (nodes_res, pods_res) {
                    (Ok(nodes), Ok(pods)) => {
                        // [수정] 여기서부터 상세 데이터를 모두 채워넣습니다.
                        let nodes_info: Vec<NodeInfo> = nodes.items.iter().map(|node| {
                            let node_name = node.name_any();
                            let mut container_count_for_node = 0;

                            let node_pods: Vec<PodInfo> = pods.items.iter()
                                .filter(|p| p.spec.as_ref().and_then(|s| s.node_name.as_ref()) == Some(&node_name))
                                .map(|p| {
                                    let containers: Vec<ContainerInfo> = p.spec.as_ref().map(|s| s.containers.iter().map(|c| {
                                        ContainerInfo {
                                            name: c.name.clone(),
                                            image: c.image.clone().unwrap_or_default(),
                                        }
                                    }).collect()).unwrap_or_default();
                                    
                                    container_count_for_node += containers.len();

                                    PodInfo {
                                        name: p.name_any(),
                                        namespace: p.namespace().unwrap_or_default(),
                                        node_name: node_name.clone(),
                                        labels: p.labels().clone(),
                                        containers,
                                        cluster_name: name.clone(),
                                    }
                                }).collect();

                            let node_status = node.status.as_ref();
                            let node_info_details = node_status.and_then(|s| s.node_info.as_ref());
                            let node_labels = node.labels().clone();

                            // [추가] GPU 관련 정보 수집 로직 복원
                            let gpu_model = node_labels.get("nvidia.com/gpu.product").cloned().unwrap_or_else(|| "N/A".to_string());
                            let gpu_count = node_status
                                .and_then(|s| s.capacity.as_ref())
                                .and_then(|c| c.get("nvidia.com/gpu").map(|q| q.0.clone()))
                                .unwrap_or_else(|| "0".to_string());

                            let mut mig_devices = HashMap::new();
                            if let Some(capacity) = node_status.and_then(|s| s.capacity.as_ref()) {
                                for (key, value) in capacity {
                                    if key.starts_with("nvidia.com/mig-") {
                                        let mig_profile = key.strip_prefix("nvidia.com/mig-").unwrap_or(key);
                                        mig_devices.insert(mig_profile.to_string(), value.0.clone());
                                    }
                                }
                            }

                            NodeInfo {
                                name: node_name.clone(),
                                labels: node.labels().clone(),
                                pods: node_pods.clone(),
                                pod_count: node_pods.len(),
                                container_count: container_count_for_node,
                                cluster_name: name.clone(),
                                os_image: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.os_image.clone()),
                                kubelet_version: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.kubelet_version.clone()),
                                architecture: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.architecture.clone()),
                                capacity_cpu: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("cpu").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()),
                                capacity_memory: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("memory").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()),
                                gpu_model,
                                gpu_count,
                                mig_devices,
                            }
                        }).collect();

                        let cluster_info = ClusterInfo {
                            name: name.clone(),
                            node_count: nodes.items.len(),
                            pod_count: pods.items.len(),
                            nodes: nodes_info,
                        };
                        
                        session_addr_clone.do_send(cluster_info);
                    }
                    (Err(e), _) => error!("[{}] 노드 조회 실패: {}", name, e),
                    (_, Err(e)) => error!("[{}] 파드 조회 실패: {}", name, e),
                }
            });
        }
    }
}

impl Actor for WsServer {
    type Context = Context<Self>;
}

// --- 웹소켓 연결 핸들러 ---
async fn ws_index(
    req: HttpRequest,
    stream: web::Payload,
    srv: web::Data<Addr<WsServer>>,
) -> Result<HttpResponse, Error> {
    let session = WsSession {
        server_addr: srv.get_ref().clone(),
    };
    ws::start(session, &req, stream)
}

// --- 메인 함수 ---
#[derive(Clone)]
pub struct ClusterConfig {
    pub name: String,
    pub kubeconfig: Kubeconfig,
    pub id_token: String,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    // --- 실제 클러스터 정보 설정 ---
    let cluster_configs = match Kubeconfig::read() {
        Ok(kubeconfig) => {
            info!("Kubeconfig 파일을 성공적으로 읽었습니다.");
            kubeconfig.contexts.iter().map(|ctx| {
                ClusterConfig {
                    name: ctx.name.clone(),
                    kubeconfig: kubeconfig.clone(),
                    // 중요: 각 컨텍스트에 맞는 ID 토큰을 제공해야 합니다.
                    id_token: "여기에_ID_토큰을_붙여넣으세요".into(),
                }
            }).collect()
        }
        Err(e) => {
            error!("Kubeconfig 파일을 읽는 데 실패했습니다: {}", e);
            vec![]
        }
    };

    if cluster_configs.is_empty() {
        warn!("설정된 클러스터 정보가 없습니다.");
    }

    // --- 클라이언트 생성 ---
    let user_clients = Arc::new(Mutex::new(HashMap::new()));
    for config in &cluster_configs {
        let name = config.name.clone();
        let kubeconfig = config.kubeconfig.clone();
        let id_token = config.id_token.clone();
        let user_clients_clone = user_clients.clone();

        tokio::spawn(async move {
            info!("[{}] 클라이언트 생성을 시도합니다...", name);
            let options = KubeConfigOptions { context: Some(name.clone()), ..Default::default() };
            match Config::from_custom_kubeconfig(kubeconfig, &options).await {
                Ok(mut config) => {
                    config.auth_info.token = Some(id_token.into());
                    config.auth_info.exec = None;
                    match Client::try_from(config) {
                        Ok(client) => {
                            user_clients_clone.lock().unwrap().insert(name.clone(), client);
                            info!("[{}] 클라이언트 생성 성공", name);
                        }
                        Err(e) => error!("[{}] Client 생성 실패: {}", name, e),
                    }
                }
                Err(e) => error!("[{}] Config 생성 실패: {}", name, e),
            }
        });
    }

    // --- 서버 시작 ---
    let server = WsServer { user_clients };
    let server_addr = server.start();

    info!("웹 서버를 http://127.0.0.1:8080 에서 시작합니다.");

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(server_addr.clone()))
            .route("/ws/", web::get().to(ws_index))
            .service(fs::Files::new("/", "./static").index_file("index.html"))
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}
    