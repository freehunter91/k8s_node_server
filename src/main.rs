use actix::{Actor, Addr, Context, Handler, Message, StreamHandler, ActorContext, AsyncContext};
use actix_files as fs;
use actix_web::{web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::{
    api::{ListParams, ResourceExt},
    Client, Config,
    config::{Kubeconfig, AuthProviderConfig},
};
use reqwest::Client as ReqwestClient;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use log::{error, info, warn};

// --- 데이터 모델 ---
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

// --- Keycloak 토큰 갱신 로직 ---
#[derive(Deserialize)]
struct OidcTokenResponse {
    id_token: String,
}

/// Keycloak 서버에 refresh_token을 보내 새 id_token을 받아오는 함수
async fn refresh_oidc_token(
    auth_provider: &AuthProviderConfig,
) -> Result<String, Box<dyn std::error::Error>> {
    let config = &auth_provider.config;
    let issuer_url = config.get("idp-issuer-url").ok_or("idp-issuer-url not found")?;
    let client_id = config.get("client-id").ok_or("client-id not found")?;
    let refresh_token = config.get("refresh-token").ok_or("refresh-token not found")?;
    let client_secret = config.get("client-secret"); // 선택 사항

    let token_url = format!("{}/protocol/openid-connect/token", issuer_url);
    
    let mut params = HashMap::new();
    params.insert("grant_type", "refresh_token");
    params.insert("client_id", client_id);
    params.insert("refresh_token", refresh_token);
    if let Some(secret) = client_secret {
        params.insert("client_secret", secret);
    }

    let client = ReqwestClient::new();
    let res = client.post(&token_url).form(&params).send().await?;

    if res.status().is_success() {
        let token_res: OidcTokenResponse = res.json().await?;
        Ok(token_res.id_token)
    } else {
        let error_text = res.text().await?;
        Err(format!("Failed to refresh token: {}", error_text).into())
    }
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
    pub user_clients: Arc<Mutex<HashMap<String, Client>>>,
}

impl Handler<FetchData> for WsServer {
    type Result = ();

    fn handle(&mut self, msg: FetchData, _ctx: &mut Self::Context) {
        let clients = self.user_clients.lock().unwrap().clone();
        let session_addr = msg.0;

        for (name, client) in clients {
            let session_addr_clone = session_addr.clone();
            tokio::spawn(async move {
                if let Some(info) = fetch_cluster_data(name.clone(), client).await {
                    session_addr_clone.do_send(info);
                }
            });
        }
    }
}

impl Actor for WsServer {
    type Context = Context<Self>;
}

// --- 데이터 조회 로직 ---
async fn fetch_cluster_data(name: String, client: Client) -> Option<ClusterInfo> {
    info!("[{}] 클러스터 정보 조회 시작...", name);
    let nodes_api: kube::Api<Node> = kube::Api::all(client.clone());
    let pods_api: kube::Api<Pod> = kube::Api::all(client);

    let lp = ListParams::default();
    let nodes_res = nodes_api.list(&lp).await;
    let pods_res = pods_api.list(&lp).await;

    match (nodes_res, pods_res) {
        (Ok(nodes), Ok(pods)) => {
            let nodes_info: Vec<NodeInfo> = nodes.items.iter().map(|node| {
                let node_name = node.name_any();
                let mut container_count_for_node = 0;

                let node_pods: Vec<PodInfo> = pods.items.iter()
                    .filter(|p| p.spec.as_ref().and_then(|s| s.node_name.as_ref()) == Some(&node_name))
                    .map(|p| {
                        let containers: Vec<ContainerInfo> = p.spec.as_ref().map(|s| s.containers.iter().map(|c| ContainerInfo {
                            name: c.name.clone(),
                            image: c.image.clone().unwrap_or_default(),
                        }).collect()).unwrap_or_default();
                        container_count_for_node += containers.len();
                        PodInfo { name: p.name_any(), namespace: p.namespace().unwrap_or_default(), node_name: node_name.clone(), labels: p.labels().clone(), containers, cluster_name: name.clone() }
                    }).collect();

                let node_status = node.status.as_ref();
                let node_info_details = node_status.and_then(|s| s.node_info.as_ref());
                let node_labels = node.labels().clone();
                let gpu_model = node_labels.get("nvidia.com/gpu.product").cloned().unwrap_or_else(|| "N/A".to_string());
                let gpu_count = node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("nvidia.com/gpu").map(|q| q.0.clone())).unwrap_or_else(|| "0".to_string());
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
                    name: node_name.clone(), labels: node_labels, pods: node_pods.clone(), pod_count: node_pods.len(), container_count: container_count_for_node, cluster_name: name.clone(),
                    os_image: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.os_image.clone()),
                    kubelet_version: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.kubelet_version.clone()),
                    architecture: node_info_details.map_or_else(|| "N/A".to_string(), |ni| ni.architecture.clone()),
                    capacity_cpu: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("cpu").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()),
                    capacity_memory: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("memory").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()),
                    gpu_model, gpu_count, mig_devices,
                }
            }).collect();

            Some(ClusterInfo { name: name.clone(), node_count: nodes.items.len(), pod_count: pods.items.len(), nodes: nodes_info })
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

    let user_clients = Arc::new(Mutex::new(HashMap::new()));

    for context in &kubeconfig.contexts {
        let context_name = &context.name;
        let user_name = &context.context.user;

        if let Some(named_auth) = kubeconfig.auth_infos.iter().find(|&u| &u.name == user_name) {
            if let Some(auth_provider) = &named_auth.auth_info.auth_provider {
                
                let user_clients_clone = user_clients.clone();
                let kubeconfig_clone = kubeconfig.clone();
                let context_name_clone = context_name.clone();
                let auth_provider_clone = auth_provider.clone();

                tokio::spawn(async move {
                    info!("[{}] OIDC 토큰 갱신을 시도합니다...", context_name_clone);
                    
                    match refresh_oidc_token(&auth_provider_clone).await {
                        Ok(new_id_token) => {
                            info!("[{}] OIDC 토큰 갱신 성공.", context_name_clone);
                            
                            let options = kube::config::KubeConfigOptions { context: Some(context_name_clone.clone()), ..Default::default() };
                            match Config::from_custom_kubeconfig(kubeconfig_clone, &options).await {
                                Ok(mut config) => {
                                    config.auth_info.token = Some(new_id_token.into());
                                    config.auth_info.exec = None;
                                    match Client::try_from(config) {
                                        Ok(client) => {
                                            user_clients_clone.lock().unwrap().insert(context_name_clone.clone(), client);
                                            info!("[{}] 새로운 토큰으로 클라이언트 생성 성공.", context_name_clone);
                                        }
                                        Err(e) => error!("[{}] Client 생성 실패: {}", context_name_clone, e),
                                    }
                                }
                                Err(e) => error!("[{}] Config 생성 실패: {}", context_name_clone, e),
                            }
                        },
                        Err(e) => {
                            warn!("[{}] OIDC 토큰 갱신 실패: {}. 이 클러스터는 제외됩니다.", context_name_clone, e);
                        }
                    }
                });
            }
        }
    }
    
    let server = WsServer { user_clients };
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

