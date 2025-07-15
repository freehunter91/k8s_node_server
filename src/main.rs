// actix 및 웹소켓 관련 라이브러리 추가
use actix::{Actor, Addr, AsyncContext, Handler, Message, StreamHandler};
use actix_cors::Cors;
use actix_files as fs;
use actix_web::{web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
use k8s_openapi::{
    api::core::v1::{Node, Pod},
};
use kube::{
    api::{ListParams, ResourceExt},
    config::{KubeConfigOptions, Kubeconfig},
    Api, Client, Config,
    Error as KubeError,
};
use log::{info, warn, error, debug};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use std::env;
use tokio::time::sleep;


// --- 데이터 모델 수정 ---
#[derive(Serialize, Deserialize, Clone, Debug, Message)]
#[rtype(result = "()")]
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
struct ContainerInfo {
    name: String,
    image: String,
}

// --- 유틸리티 함수 ---
fn log_kube_error(context_name: &str, action: &str, e: &KubeError) {
    error!("❌ [Context: {}] {} 실패: {}", context_name, action, e);
    match e {
        KubeError::Api(api_error) => {
            error!("   API 오류 상세: Status={}, Message={}", api_error.code, api_error.message);
            if api_error.code == 401 || api_error.code == 403 {
                error!("   인증/권한 오류 가능성: 토큰 만료, 잘못된 토큰, 또는 권한 부족.");
            }
        },
        KubeError::InferConfig(config_error) => {
            error!("   Kubeconfig 추론/로드 오류 상세: {:?}", config_error);
        }
        _ => error!("   기타 kube-rs 오류 (자세한 내용은 오류 메시지 확인): {:?}", e),
    }
}

// [수정] 컨테이너 환경에서 메모리 사용 '사용률'을 측정하는 함수
async fn log_self_memory_usage() {
    let log_interval = Duration::from_secs(60); // 60초 간격으로 로그 출력

    loop {
        // 컨테이너 환경(cgroup v1)에서 메모리 사용량/사용률을 먼저 시도
        let memory_info = match std::fs::read_to_string("/sys/fs/cgroup/memory/memory.usage_in_bytes") {
            Ok(usage_str) => {
                let usage_bytes = usage_str.trim().parse::<f64>().unwrap_or(0.0);
                
                // 메모리 제한 값을 읽어 사용률 계산
                match std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes") {
                    Ok(limit_str) => {
                        let limit_bytes = limit_str.trim().parse::<f64>().unwrap_or(0.0);
                        
                        // memory.limit_in_bytes가 너무 크면 (사실상 제한 없음), 사용률 계산이 무의미
                        // Kubernetes가 설정하는 일반적인 최대값보다 훨씬 큰 값으로 체크 (예: 2^60)
                        if limit_bytes > 1_000_000_000_000_000_000.0 {
                             format!("{:.2} MB (cgroup, 제한 없음)", usage_bytes / 1024.0 / 1024.0)
                        } else {
                            let usage_percent = if limit_bytes > 0.0 { (usage_bytes / limit_bytes) * 100.0 } else { 0.0 };
                            format!(
                                "{:.2}% ({:.2} MB / {:.2} MB) (cgroup)",
                                usage_percent,
                                usage_bytes / 1024.0 / 1024.0,
                                limit_bytes / 1024.0 / 1024.0
                            )
                        }
                    },
                    Err(_) => {
                        // 제한 값을 읽지 못하면 사용량만 표시
                        format!("{:.2} MB (cgroup, 제한 값 읽기 실패)", usage_bytes / 1024.0 / 1024.0)
                    }
                }
            },
            Err(_) => {
                // cgroup 파일이 없으면, /proc/self/status에서 VmRSS를 읽음 (사용률 계산 불가)
                match std::fs::read_to_string("/proc/self/status") {
                    Ok(content) => {
                        let mut vm_rss = "N/A".to_string();
                        for line in content.lines() {
                            if line.starts_with("VmRSS:") {
                                if let Some(value) = line.split_whitespace().nth(1) {
                                    if let Ok(kb) = value.parse::<f64>() {
                                        vm_rss = format!("{:.2} MB (VmRSS)", kb / 1024.0);
                                    }
                                }
                                break;
                            }
                        }
                        vm_rss
                    }
                    Err(_) => "N/A (메모리 정보 읽기 실패)".to_string()
                }
            }
        };

        info!("🧠 백엔드 메모리 사용률: {}", memory_info);
        sleep(log_interval).await;
    }
}


// --- Actor 및 메시지 정의 ---
#[derive(Message)]
#[rtype(result = "()")]
struct FetchClusterInfo;
struct MyWebSocket { kube_contexts: Arc<KubeContexts> }
impl Actor for MyWebSocket {
    type Context = ws::WebsocketContext<Self>;
    fn started(&mut self, ctx: &mut Self::Context) {
        info!("WebSocket 연결 시작됨.");
        ctx.address().do_send(FetchClusterInfo);
        ctx.run_interval(Duration::from_secs(300), |_, ctx| {
            info!("주기적인 클러스터 정보 조회 트리거됨.");
            ctx.address().do_send(FetchClusterInfo);
        });
    }
}
impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for MyWebSocket {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(msg)) => ctx.pong(&msg),
            _ => (),
        }
    }
}
impl Handler<ClusterInfo> for MyWebSocket {
    type Result = ();
    fn handle(&mut self, msg: ClusterInfo, ctx: &mut Self::Context) {
        if let Ok(json_str) = serde_json::to_string(&msg) {
            ctx.text(json_str);
        } else {
            warn!("ClusterInfo를 JSON으로 직렬화하는 데 실패했습니다.");
        }
    }
}
impl Handler<FetchClusterInfo> for MyWebSocket {
    type Result = ();
    fn handle(&mut self, _msg: FetchClusterInfo, ctx: &mut Self::Context) {
        let addr = ctx.address();
        let contexts_clone = self.kube_contexts.clone();
        actix_web::rt::spawn(async move {
            fetch_and_stream_data(contexts_clone, addr).await;
        });
    }
}

// --- 데이터 수집 로직 ---
async fn fetch_and_stream_data(kube_contexts: Arc<KubeContexts>, addr: Addr<MyWebSocket>) {
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
                    let node_name = node.name_any();
                    let node_pods: Vec<PodInfo> = pods.items.iter()
                        .filter(|p| p.spec.as_ref().and_then(|s| s.node_name.as_ref()) == Some(&node_name))
                        .map(|p| {
                            let pod_name = p.name_any();
                            
                            let containers: Vec<ContainerInfo> = p.spec.as_ref().map(|s| s.containers.iter().map(|c| {
                                ContainerInfo {
                                    name: c.name.clone(),
                                    image: c.image.clone().unwrap_or_default(),
                                }
                            }).collect()).unwrap_or_default();
                            
                            PodInfo {
                                name: pod_name,
                                namespace: p.namespace().unwrap_or_default(),
                                node_name: node_name.clone(),
                                labels: p.labels().clone(),
                                containers,
                                cluster_name: context_name.clone(),
                            }
                        }).collect();

                    let container_count = node_pods.iter().map(|p| p.containers.len()).sum();
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
                    nodes_info.push(NodeInfo { name: node_name, labels: node_labels, pod_count: node_pods.len(), container_count, pods: node_pods, cluster_name: context_name.clone(), os_image: node_info_details.map_or("N/A".to_string(), |ni| ni.os_image.clone()), kubelet_version: node_info_details.map_or("N/A".to_string(), |ni| ni.kubelet_version.clone()), architecture: node_info_details.map_or("N/A".to_string(), |ni| ni.architecture.clone()), capacity_cpu: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("cpu").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()), capacity_memory: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("memory").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()), gpu_model, gpu_count, mig_devices });
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
            (nodes_res, pods_res) => {
                if let Err(e) = nodes_res { log_kube_error(context_name, "노드 조회", &e); }
                if let Err(e) = pods_res { log_kube_error(context_name, "파드 조회", &e); }
                warn!("⚠️ [Context: {}] 정보 조회에 실패하여 건너뜁니다.", context_name);
            }
        }
    }
    info!("모든 클러스터 정보 조회를 완료했습니다.");
}

async fn ws_route(req: HttpRequest, stream: web::Payload, kube_contexts: web::Data<Arc<KubeContexts>>) -> Result<HttpResponse, Error> {
    ws::start(MyWebSocket { kube_contexts: kube_contexts.get_ref().clone() }, &req, stream)
}
struct KubeContexts { contexts: HashMap<String, Client> }
#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));
    info!("K8s 대시보드 백엔드 서버 초기화 중...");
    
    tokio::spawn(log_self_memory_usage());

    let mut contexts = HashMap::new();
    if let Ok(config) = Kubeconfig::read() {
        debug!("Kubeconfig 파일 읽기 성공.");
        for context in &config.contexts {
            let context_name = &context.name;
            let options = KubeConfigOptions { context: Some(context_name.clone()), ..Default::default() };
            match Config::from_custom_kubeconfig(config.clone(), &options).await {
                Ok(config_for_context) => {
                    match Client::try_from(config_for_context) {
                        Ok(client) => { contexts.insert(context_name.clone(), client); },
                        Err(e) => log_kube_error(context_name, "클라이언트 생성", &e),
                    }
                },
                Err(e) => { error!("❌ [Context: {}] Kubeconfig로부터 설정 로드 실패: {}", context_name, e); }
            }
        }
    } else {
        let kubeconfig_path = env::var("KUBECONFIG").unwrap_or_else(|_| "~/.kube/config".to_string());
        error!("🚨 Kubeconfig 파일을 읽는 데 실패했습니다. 경로: '{}' 또는 파일 권한을 확인하세요.", kubeconfig_path);
    }
    info!("--- 클러스터 사전 접속 테스트 시작 ---");
    let mut successfully_connected_contexts = HashMap::new();
    if contexts.is_empty() {
        warn!("사전 테스트할 클러스터가 없습니다.");
    } else {
        for (context_name, client) in contexts.drain() {
            let nodes_api: Api<Node> = Api::all(client.clone());
            match nodes_api.list(&ListParams::default().limit(1)).await {
                Ok(_) => {
                    info!("✅ [Context: {}] Kubernetes API 서버에 성공적으로 접속했습니다.", context_name);
                    successfully_connected_contexts.insert(context_name.clone(), client);
                },
                Err(e) => { log_kube_error(&context_name, "Kubernetes API 서버 접속 테스트", &e); }
            }
            sleep(Duration::from_secs(3)).await;
        }
    }
    info!("--- 클러스터 사전 접속 테스트 완료 ---");
    let kube_contexts = web::Data::new(Arc::new(KubeContexts { contexts: successfully_connected_contexts }));
    if kube_contexts.contexts.is_empty() {
        error!("🚨 접속 가능한 Kubernetes 클러스터가 없습니다. 서버를 시작하지 않습니다.");
        return Ok(());
    }
    info!("\n🚀 서버 시작: http://0.0.0.0:8080");

    HttpServer::new(move || {
        App::new()
            .wrap(Cors::default().allow_any_origin().allowed_methods(vec!["GET"]).allow_any_header().max_age(3600))
            .app_data(kube_contexts.clone())
            .route("/ws/", web::get().to(ws_route))
            .service(fs::Files::new("/", "./static").index_file("index.html"))
    })
    .bind(("0.0.0.0", 8080))?
    .run()
    .await
}
