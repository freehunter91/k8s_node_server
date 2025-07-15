// actix 및 웹소켓 관련 라이브러리 추가
use actix::{Actor, Addr, AsyncContext, Handler, Message, StreamHandler, ActorContext};
use actix_cors::Cors;
use actix_files as fs;
use actix_web::{web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::{
    api::ListParams,
    config::{KubeConfigOptions, Kubeconfig},
    Api, Client, Config,
};
use log::{info, warn, error, debug};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use std::env; // 환경 변수 접근을 위해 추가
use tokio::time::sleep; // 비동기 대기 시간을 위해 추가

// --- 데이터 모델 (변경 없음) ---
#[derive(Serialize, Deserialize, Clone, Debug, Message)]
#[rtype(result = "()")] // 이 구조체를 Actor 메시지로 사용하기 위해 추가
struct ClusterInfo { name: String, nodes: Vec<NodeInfo>, node_count: usize, pod_count: usize }

#[derive(Serialize, Deserialize, Clone, Debug)]
struct NodeInfo {
    name: String,
    labels: BTreeMap<String, String>,
    pods: Vec<PodInfo>,
    pod_count: usize,
    container_count: usize,
    cluster_name: String,
    os_image: String,
    kubelet_version: String,
    architecture: String,
    capacity_cpu: String,
    capacity_memory: String,
    gpu_model: String,
    gpu_count: String,
    mig_devices: HashMap<String, String>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
struct PodInfo {
    name: String,
    namespace: String,
    node_name: String,
    labels: BTreeMap<String, String>,
    containers: Vec<ContainerInfo>,
    cluster_name: String,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
struct ContainerInfo { name: String, image: String }

// 주기적인 클러스터 정보 조회를 위한 메시지 정의
#[derive(Message)]
#[rtype(result = "()")]
struct FetchClusterInfo;

/// WebSocket 연결을 처리할 Actor 정의
struct MyWebSocket {
    kube_contexts: Arc<KubeContexts>,
}

impl Actor for MyWebSocket {
    type Context = ws::WebsocketContext<Self>;

    /// 웹소켓 연결이 시작될 때 호출되는 메소드
    fn started(&mut self, ctx: &mut Self::Context) {
        info!("WebSocket 연결 시작됨.");

        // 초기 데이터 조회 요청
        ctx.address().do_send(FetchClusterInfo);

        // 매 5분(300초)마다 FetchClusterInfo 메시지를 자신에게 보내도록 스케줄링
        ctx.run_interval(Duration::from_secs(300), |act, ctx| {
            info!("주기적인 클러스터 정보 조회 트리거됨.");
            ctx.address().do_send(FetchClusterInfo); // MyWebSocket 액터 자신에게 메시지 전송
        });
    }
}

/// 클라이언트로부터 들어오는 웹소켓 메시지를 처리
impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for MyWebSocket {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(msg)) => ctx.pong(&msg),
            Ok(ws::Message::Text(text)) => info!("수신된 텍스트: {}", text),
            Ok(ws::Message::Close(reason)) => {
                ctx.close(reason);
                ctx.stop();
            }
            _ => (),
        }
    }
}

/// ClusterInfo 메시지를 받았을 때 처리하는 핸들러
impl Handler<ClusterInfo> for MyWebSocket {
    type Result = ();

    fn handle(&mut self, msg: ClusterInfo, ctx: &mut Self::Context) {
        // 받은 ClusterInfo를 JSON 문자열로 변환하여 클라이언트에게 전송
        if let Ok(json_str) = serde_json::to_string(&msg) {
            ctx.text(json_str);
        } else {
            warn!("ClusterInfo를 JSON으로 직렬화하는 데 실패했습니다.");
        }
    }
}

// FetchClusterInfo 메시지를 처리하는 핸들러 추가
impl Handler<FetchClusterInfo> for MyWebSocket {
    type Result = ();

    fn handle(&mut self, _msg: FetchClusterInfo, ctx: &mut Self::Context) {
        let addr = ctx.address();
        let contexts_clone = self.kube_contexts.clone();

        // 데이터 조회는 오래 걸릴 수 있으므로, 별도의 비동기 작업으로 분리
        actix_web::rt::spawn(async move {
            fetch_and_stream_data(contexts_clone, addr).await;
        });
    }
}


/// 클러스터 정보를 조회하고 웹소켓 액터에게 메시지를 보내는 함수
async fn fetch_and_stream_data(kube_contexts: Arc<KubeContexts>, addr: Addr<MyWebSocket>) {
    // KubeContexts가 비어있는지 확인 (사전 테스트에서 모두 실패한 경우)
    if kube_contexts.contexts.is_empty() {
        warn!("⚠️ KubeContexts에 유효한 클러스터가 없어 정보 조회를 건너뜁니다.");
        return;
    }

    for (context_name, client) in &kube_contexts.contexts {
        info!(">>> [Context: {}] 클러스터 정보 조회 시작...", context_name);
        let nodes_api: Api<Node> = Api::all(client.clone());
        let pods_api: Api<Pod> = Api::all(client.clone());
        let lp = ListParams::default();

        let nodes_res = nodes_api.list(&lp).await;
        let pods_res = pods_api.list(&lp).await;

        match (nodes_res, pods_res) {
            (Ok(nodes), Ok(pods)) => {
                debug!("[Context: {}] 노드 및 파드 데이터 성공적으로 수신.", context_name);
                let mut nodes_info = vec![];
                for node in nodes.items {
                    let node_name = node.metadata.name.clone().unwrap_or_default();
                    let node_pods: Vec<PodInfo> = pods.items.iter()
                        .filter(|p| p.spec.as_ref().and_then(|s| s.node_name.as_ref()) == Some(&node_name))
                        .map(|p| {
                            let containers = p.spec.as_ref().map(|s| s.containers.iter().map(|c| ContainerInfo {
                                name: c.name.clone(),
                                image: c.image.clone().unwrap_or_default(),
                            }).collect()).unwrap_or_default();
                            PodInfo {
                                name: p.metadata.name.clone().unwrap_or_default(),
                                namespace: p.metadata.namespace.clone().unwrap_or_default(),
                                node_name: node_name.clone(),
                                labels: p.metadata.labels.clone().unwrap_or_default(),
                                containers,
                                cluster_name: context_name.clone(),
                            }
                        }).collect();

                    let container_count = node_pods.iter().map(|p| p.containers.len()).sum();
                    let node_status = node.status.as_ref();
                    let node_info_details = node_status.and_then(|s| s.node_info.as_ref());
                    let node_labels = node.metadata.labels.clone().unwrap_or_default();

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
                    nodes_info.push(NodeInfo { name: node_name.clone(), labels: node_labels.clone(), pod_count: node_pods.len(), container_count, pods: node_pods, cluster_name: context_name.clone(), os_image: node_info_details.map_or("N/A".to_string(), |ni| ni.os_image.clone()), kubelet_version: node_info_details.map_or("N/A".to_string(), |ni| ni.kubelet_version.clone()), architecture: node_info_details.map_or("N/A".to_string(), |ni| ni.architecture.clone()), capacity_cpu: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("cpu").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()), capacity_memory: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("memory").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()), gpu_model, gpu_count, mig_devices });
                }

                let cluster_info = ClusterInfo {
                    name: context_name.clone(),
                    node_count: nodes_info.len(),
                    pod_count: nodes_info.iter().map(|n| n.pod_count).sum(),
                    nodes: nodes_info,
                };

                addr.do_send(cluster_info);
                info!("✅ [Context: {}] 클러스터 정보를 클라이언트로 전송했습니다.", context_name);
            },
            (nodes_res, pods_res) => { // 노드 또는 파드 조회 중 하나라도 실패한 경우
                let mut error_messages = String::new();
                if let Err(nodes_err) = nodes_res {
                    error_messages.push_str(&format!("노드 조회 실패: {}", nodes_err));
                    // 401 Unauthorized 오류에 대한 특별 로깅
                    if let Some(api_error) = nodes_err.as_api_error() {
                        if api_error.code == 401 {
                            error!("❌ [Context: {}] 인증 오류 (401 Unauthorized) - 노드 조회. kubeconfig 토큰 또는 권한을 확인하세요.", context_name);
                            error!("   API 오류 상세: Status={:?}, Message={}", api_error.status, api_error.message);
                        }
                    }
                    if let Some(req_error) = nodes_err.as_request_error() {
                        error!("   요청 오류 상세: {:?}", req_error);
                    }
                    if let Some(io_error) = nodes_err.as_io_error() {
                        error!("   IO 오류 상세: {:?}", io_error);
                    }
                }
                if let Err(pods_err) = pods_res {
                    if !error_messages.is_empty() { error_messages.push_str("; "); }
                    error_messages.push_str(&format!("파드 조회 실패: {}", pods_err));
                    // 401 Unauthorized 오류에 대한 특별 로깅
                    if let Some(api_error) = pods_err.as_api_error() {
                        if api_error.code == 401 {
                            error!("❌ [Context: {}] 인증 오류 (401 Unauthorized) - 파드 조회. kubeconfig 토큰 또는 권한을 확인하세요.", context_name);
                            error!("   API 오류 상세: Status={:?}, Message={}", api_error.status, api_error.message);
                        }
                    }
                    if let Some(req_error) = pods_err.as_request_error() {
                        error!("   요청 오류 상세: {:?}", req_error);
                    }
                    if let Some(io_error) = pods_err.as_io_error() {
                        error!("   IO 오류 상세: {:?}", io_error);
                    }
                }
                error!("❌ [Context: {}] 클러스터 정보 조회 실패: {}", context_name, error_messages);
                warn!("⚠️ [Context: {}] 정보 조회에 실패하여 건너뜁니다.", context_name);
            }
        }
    }
    info!("모든 클러스터 정보 조회를 완료했습니다.");
}

/// 웹소켓 요청을 처리하는 HTTP 핸들러
async fn ws_route(
    req: HttpRequest,
    stream: web::Payload,
    kube_contexts: web::Data<Arc<KubeContexts>>,
) -> Result<HttpResponse, Error> {
    ws::start(
        MyWebSocket {
            kube_contexts: kube_contexts.get_ref().clone(),
        },
        &req,
        stream,
    )
}

struct KubeContexts {
    contexts: HashMap<String, Client>,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));
    info!("K8s 대시보드 백엔드 서버 초기화 중...");

    let mut contexts = HashMap::new();

    // 1. Kubeconfig 파일로부터 클러스터 정보 로드
    if let Ok(config) = Kubeconfig::read() {
        debug!("Kubeconfig 파일 읽기 성공.");
        for context in &config.contexts {
            let context_name = &context.name;
            debug!("컨텍스트 '{}' 로드 시도...", context_name);
            let options = KubeConfigOptions { context: Some(context_name.clone()), ..Default::default() };
            match Config::from_custom_kubeconfig(config.clone(), &options).await {
                Ok(config_for_context) => {
                    match Client::try_from(config_for_context) {
                        Ok(client) => {
                            debug!("클라이언트 '{}' 생성 성공.", context_name);
                            contexts.insert(context_name.clone(), client);
                        },
                        Err(e) => {
                            warn!("⚠️ [Context: {}] 클라이언트 생성 실패: {}", context_name, e);
                            if let Some(auth_err) = e.as_auth_error() {
                                error!("   인증 오류 상세: {:?}", auth_err);
                            } else if let Some(config_err) = e.as_config_error() {
                                error!("   설정 오류 상세: {:?}", config_err);
                            }
                        }
                    }
                },
                Err(e) => {
                    warn!("⚠️ [Context: {}] 설정 로드 실패: {}", context_name, e);
                    if let Some(config_err) = e.as_config_error() {
                        error!("   설정 오류 상세: {:?}", config_err);
                    }
                }
            }
        }
    } else {
        let kubeconfig_path = env::var("KUBECONFIG").unwrap_or_else(|_| "~/.kube/config".to_string());
        error!("🚨 Kubeconfig 파일을 읽는 데 실패했습니다. 경로: '{}' 또는 파일 권한을 확인하세요.", kubeconfig_path);
    }

    // 2. 환경 변수로부터 외부 토큰 인증 클러스터 정보 로드
    let external_cluster_name = env::var("K8S_EXTERNAL_CLUSTER_NAME");
    let external_cluster_url = env::var("K8S_EXTERNAL_CLUSTER_URL");
    let external_token = env::var("K8S_EXTERNAL_TOKEN");
    let external_ca_cert_path = env::var("K8S_EXTERNAL_CA_CERT_PATH"); // 선택적 CA 인증서 경로

    if let (Ok(name), Ok(url), Ok(token)) = (external_cluster_name, external_cluster_url, external_token) {
        info!("환경 변수로부터 외부 클러스터 정보 감지: '{}'", name);
        let mut external_config = Config::new(url.parse().expect("K8S_EXTERNAL_CLUSTER_URL이 유효한 URL 형식이 아닙니다."));
        external_config.token = Some(token);

        // 외부 CA 인증서 경로가 제공되면 로드
        if let Ok(ca_path) = external_ca_cert_path {
            match std::fs::read(ca_path) {
                Ok(ca_data) => {
                    external_config.root_cert = Some(ca_data);
                    info!("외부 클러스터 '{}'를 위해 CA 인증서 로드 성공.", name);
                },
                Err(e) => {
                    error!("외부 클러스터 '{}'를 위한 CA 인증서 파일 읽기 실패 ({}): {}", name, ca_path, e);
                    // CA 인증서 로드 실패 시 해당 클러스터는 건너뜁니다.
                    // continue; // 이 루프는 단일 외부 클러스터이므로 continue 대신 경고만 출력
                }
            }
        } else {
            warn!("K8S_EXTERNAL_CA_CERT_PATH 환경 변수가 설정되지 않았습니다. 자체 서명된 인증서 클러스터에 연결 시 문제가 발생할 수 있습니다.");
            // 필요에 따라 self-signed certs를 허용하려면:
            // external_config.accept_invalid_certs = true;
        }

        match Client::try_from(external_config) {
            Ok(client) => {
                info!("외부 클러스터 '{}' 클라이언트 생성 성공.", name);
                contexts.insert(name, client);
            },
            Err(e) => {
                error!("❌ 외부 클러스터 클라이언트 생성 실패 ({}): {}", name, e);
                if let Some(auth_err) = e.as_auth_error() {
                    error!("   인증 오류 상세: {:?}", auth_err);
                } else if let Some(config_err) = e.as_config_error() {
                    error!("   설정 오류 상세: {:?}", config_err);
                }
            }
        }
    } else {
        debug!("K8S_EXTERNAL_CLUSTER_NAME, K8S_EXTERNAL_CLUSTER_URL, K8S_EXTERNAL_TOKEN 환경 변수가 모두 설정되지 않아 외부 클러스터 로드를 건너뜁니다.");
    }

    // --- 사전 접속 테스트 로직 ---
    info!("--- 클러스터 사전 접속 테스트 시작 ---");
    let mut successfully_connected_contexts = HashMap::new();
    if contexts.is_empty() {
        warn!("사전 테스트할 클러스터가 없습니다. kubeconfig 설정을 확인해주세요.");
    } else {
        for (context_name, client) in contexts.drain() {
            info!("테스트 중: [Context: {}]", context_name);
            let nodes_api: Api<Node> = Api::all(client.clone());
            let lp = ListParams::default().limit(1);

            match nodes_api.list(&lp).await {
                Ok(_) => {
                    info!("✅ [Context: {}] Kubernetes API 서버에 성공적으로 접속했습니다.", context_name);
                    successfully_connected_contexts.insert(context_name, client);
                },
                Err(e) => {
                    error!("❌ [Context: {}] Kubernetes API 서버 접속 테스트 실패: {}", context_name, e);
                    if let Some(api_error) = e.as_api_error() {
                        error!("   API 오류 상세: Status={:?}, Message={}", api_error.status, api_error.message);
                        if api_error.code == 401 || api_error.code == 403 {
                            error!("   인증/권한 오류 가능성: 토큰 만료, 잘못된 토큰, 또는 권한 부족.");
                        }
                    } else if let Some(req_error) = e.as_request_error() {
                        error!("   요청 오류 상세: {:?}", req_error);
                    } else if let Some(io_error) = e.as_io_error() {
                        error!("   IO 오류 상세: {:?}", io_error);
                    } else if let Some(auth_err) = e.as_auth_error() {
                        error!("   인증 시스템 오류 상세: {:?}", auth_err);
                    } else if let Some(config_err) = e.as_config_error() {
                        error!("   설정 로드 오류 상세: {:?}", config_err);
                    } else {
                        error!("   기타 kube-rs 오류: {:?}", e);
                    }
                }
            }
            // 각 클러스터 접속 테스트 후 3초 대기
            sleep(Duration::from_secs(3)).await;
        }
    }
    info!("--- 클러스터 사전 접속 테스트 완료 ---");

    // 성공적으로 접속된 클라이언트만 사용하여 KubeContexts 생성
    let kube_contexts = web::Data::new(Arc::new(KubeContexts { contexts: successfully_connected_contexts }));

    // 만약 접속 가능한 클러스터가 하나도 없다면 서버를 시작하지 않거나 경고
    if kube_contexts.contexts.is_empty() {
        error!("🚨 접속 가능한 Kubernetes 클러스터가 없습니다. 서버를 시작하지 않습니다. kubeconfig 및 네트워크 설정을 확인해주세요.");
        return Ok(());
    }

    info!("\n🚀 서버 시작: http://127.0.0.1:8080");

    HttpServer::new(move || {
        let cors = Cors::default().allow_any_origin().allowed_methods(vec!["GET"]).allow_any_header().max_age(3600);
        App::new()
            .wrap(cors)
            .app_data(kube_contexts.clone())
            .route("/ws/", web::get().to(ws_route))
            .service(fs::Files::new("/", "./static").index_file("index.html"))
    })
    .bind(("0.0.0.0", 8080))?
    .run()
    .await
}
