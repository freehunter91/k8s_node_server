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
use chrono;

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

// --- 웹소켓 세션 액터 ---
pub struct WsSession {
    pub user_clients: web::Data<Arc<Mutex<HashMap<String, Client>>>>,
}

impl Actor for WsSession {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("새로운 웹소켓 세션 시작됨. 5초마다 데이터 조회를 시작합니다.");
        let clients_arc = self.user_clients.get_ref().clone();
        let addr = ctx.address();

        // 5초마다 주기적으로 모든 클러스터의 데이터를 조회
        ctx.run_interval(std::time::Duration::from_secs(5), move |_act, _ctx| {
            let clients_clone = clients_arc.clone();
            let addr_clone = addr.clone();
            
            tokio::spawn(async move {
                let locked_clients = match clients_clone.lock() {
                    Ok(clients) => clients,
                    Err(poisoned) => {
                        warn!("Mutex가 독점되었습니다. 복구를 시도합니다.");
                        poisoned.into_inner()
                    }
                };

                let mut fetch_futures = vec![];
                for (name, client) in locked_clients.iter() {
                    let name_clone = name.clone();
                    let client_clone = client.clone();
                    fetch_futures.push(tokio::spawn(async move {
                        fetch_cluster_data(name_clone, client_clone).await
                    }));
                }
                
                // 모든 클러스터 정보를 병렬로 가져옴
                drop(locked_clients); // 빠른 락 해제
                
                let results = join_all(fetch_futures).await;
                for result in results {
                    match result {
                        Ok(Some(info)) => {
                            addr_clone.do_send(info);
                        }
                        Ok(None) => {
                            // 데이터 조회 실패 (이미 로그됨)
                        }
                        Err(e) => {
                            error!("클러스터 데이터 조회 태스크 실패: {}", e);
                        }
                    }
                }
            });
        });
    }
}

impl Handler<ClusterInfo> for WsSession {
    type Result = ();
    
    fn handle(&mut self, msg: ClusterInfo, ctx: &mut Self::Context) {
        match serde_json::to_string(&msg) {
            Ok(json) => ctx.text(json),
            Err(e) => {
                error!("ClusterInfo 직렬화 실패: {}", e);
            }
        }
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsSession {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Text(text)) => {
                info!("클라이언트로부터 메시지 수신: {}", text);
                // 필요하다면 클라이언트 메시지 처리 로직 추가
            }
            Ok(ws::Message::Binary(bin)) => {
                info!("클라이언트로부터 바이너리 데이터 수신: {} bytes", bin.len());
            }
            Ok(ws::Message::Close(reason)) => {
                info!("웹소켓 연결 종료: {:?}", reason);
                ctx.stop();
            }
            Ok(ws::Message::Ping(msg)) => {
                ctx.pong(&msg);
            }
            Ok(ws::Message::Pong(_)) => {
                // Pong 메시지는 무시
            }
            Ok(ws::Message::Continuation(_)) => {
                // Continuation 메시지는 현재 처리하지 않음
            }
            Err(e) => {
                error!("웹소켓 오류 발생: {}", e);
                ctx.stop();
            }
        }
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
                    let containers: Vec<ContainerInfo> = p.spec.as_ref()
                        .map(|s| s.containers.iter().map(|c| ContainerInfo {
                            name: c.name.clone(),
                            image: c.image.clone().unwrap_or_default(),
                        }).collect())
                        .unwrap_or_default();
                    
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
                
                NodeInfo {
                    name: node_name.clone(),
                    labels: node.labels().clone(),
                    pods: node_pods_info,
                    pod_count: node_pods_vec.len(),
                    container_count: container_count_for_node,
                    cluster_name: name.clone(),
                    os_image: node_info_details
                        .map_or_else(|| "N/A".to_string(), |ni| ni.os_image.clone()),
                    kubelet_version: node_info_details
                        .map_or_else(|| "N/A".to_string(), |ni| ni.kubelet_version.clone()),
                    architecture: node_info_details
                        .map_or_else(|| "N/A".to_string(), |ni| ni.architecture.clone()),
                    capacity_cpu: node_status
                        .and_then(|s| s.capacity.as_ref())
                        .and_then(|c| c.get("cpu").map(|q| q.0.clone()))
                        .unwrap_or_else(|| "N/A".to_string()),
                    capacity_memory: node_status
                        .and_then(|s| s.capacity.as_ref())
                        .and_then(|c| c.get("memory").map(|q| q.0.clone()))
                        .unwrap_or_else(|| "N/A".to_string()),
                }
            }).collect();
            
            Some(ClusterInfo {
                name: name.clone(),
                node_count: nodes.items.len(),
                pod_count: total_pod_count,
                nodes: nodes_info,
            })
        }
        (Err(e), _) => {
            error!("[{}] 노드 조회 실패: {}", name, e);
            None
        }
        (_, Err(e)) => {
            error!("[{}] 파드 조회 실패: {}", name, e);
            None
        }
    }
}

// --- 웹소켓 연결 핸들러 ---
async fn ws_index(
    req: HttpRequest,
    stream: web::Payload,
    clients: web::Data<Arc<Mutex<HashMap<String, Client>>>>,
) -> Result<HttpResponse, Error> {
    ws::start(WsSession { user_clients: clients }, &req, stream)
}

// --- 헬스 체크 엔드포인트 ---
async fn health_check(
    clients: web::Data<Arc<Mutex<HashMap<String, Client>>>>,
) -> Result<HttpResponse, Error> {
    let client_count = match clients.lock() {
        Ok(clients) => clients.len(),
        Err(poisoned) => {
            warn!("Mutex가 독점되었습니다. 복구를 시도합니다.");
            poisoned.into_inner().len()
        }
    };
    
    let response = serde_json::json!({
        "status": "healthy",
        "active_clusters": client_count,
        "timestamp": chrono::Utc::now().to_rfc3339()
    });
    
    Ok(HttpResponse::Ok().json(response))
}

// --- 클러스터 정보 조회 엔드포인트 ---
async fn cluster_info(
    clients: web::Data<Arc<Mutex<HashMap<String, Client>>>>,
) -> Result<HttpResponse, Error> {
    let locked_clients = match clients.lock() {
        Ok(clients) => clients,
        Err(poisoned) => {
            warn!("Mutex가 독점되었습니다. 복구를 시도합니다.");
            poisoned.into_inner()
        }
    };
    
    let cluster_names: Vec<String> = locked_clients.keys().cloned().collect();
    drop(locked_clients);
    
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "clusters": cluster_names,
        "count": cluster_names.len()
    })))
}

// --- 인증 토큰 갱신 함수 ---
async fn refresh_token_if_needed(config: &mut Config) -> Result<(), kube::Error> {
    // 실제 Keycloak 토큰 갱신 로직을 여기에 구현
    // 이는 예시이며, 실제 환경에 맞게 수정해야 합니다.
    
    // 예: 토큰 만료 시간 확인
    if let Some(token) = &config.auth_info.token {
        if token == "여기에_ID_토큰을_붙여넣으세요" {
            warn!("기본 토큰이 설정되어 있습니다. 실제 토큰으로 교체하세요.");
        }
    }
    
    Ok(())
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

    // 모든 클라이언트 생성을 비동기적으로 처리
    let mut client_creation_futures = vec![];
    
    for context in &kubeconfig.contexts {
        let context_name = context.name.clone();
        let kubeconfig_clone = kubeconfig.clone();
        
        client_creation_futures.push(tokio::spawn(async move {
            info!("[{}] 클라이언트 생성을 시도합니다...", context_name);
            
            let options = KubeConfigOptions {
                context: Some(context_name.clone()),
                ..Default::default()
            };
            
            let mut client_config = match Config::from_custom_kubeconfig(kubeconfig_clone, &options).await {
                Ok(config) => config,
                Err(e) => {
                    error!("[{}] 클라이언트 설정 실패: {}", context_name, e);
                    return Err(e);
                }
            };
            
            // 실제 환경에서는 이 부분을 적절한 토큰 관리 로직으로 교체해야 합니다.
            // 예: 환경 변수에서 토큰 읽기, OAuth 플로우, 서비스 어카운트 등
            if let Ok(token) = std::env::var("KEYCLOAK_TOKEN") {
                client_config.auth_info.token = Some(token);
            } else {
                warn!("[{}] KEYCLOAK_TOKEN 환경 변수가 설정되지 않았습니다.", context_name);
                client_config.auth_info.token = Some("여기에_ID_토큰을_붙여넣으세요".into());
            }
            
            client_config.auth_info.exec = None;

            // 토큰 갱신 시도
            if let Err(e) = refresh_token_if_needed(&mut client_config).await {
                warn!("[{}] 토큰 갱신 실패: {}", context_name, e);
            }

            let client = Client::try_from(client_config)?;
            info!("[{}] 클라이언트 생성 성공", context_name);
            
            Ok((context_name, client))
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
        error!("성공적으로 생성된 클라이언트가 없습니다. 서버를 종료합니다.");
        return Ok(());
    }
    
    let app_data = web::Data::new(Arc::new(Mutex::new(user_clients)));
    
    info!("웹 서버를 http://127.0.0.1:8080 에서 시작합니다.");
    info!("웹소켓 연결: ws://127.0.0.1:8080/ws/");
    info!("헬스 체크: http://127.0.0.1:8080/health");
    info!("클러스터 정보: http://127.0.0.1:8080/api/clusters");

    HttpServer::new(move || {
        App::new()
            .app_data(app_data.clone())
            .route("/ws/", web::get().to(ws_index))
            .route("/health", web::get().to(health_check))
            .route("/api/clusters", web::get().to(cluster_info))
            .service(fs::Files::new("/", "./static").index_file("index.html"))
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}