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
use log::{info, warn, error};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

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

        // 초기 데이터 조회 요청 (연결되자마자 한 번 실행)
        ctx.address().do_send(FetchClusterInfo);

        // 매 5분(300초)마다 FetchClusterInfo 메시지를 자신에게 보내도록 스케줄링
        ctx.run_interval(Duration::from_secs(300), |act, ctx| {
            info!("주기적인 클러스터 정보 조회 트리거됨.");
            // 수정: act.kube_contexts.clone().do_send(FetchClusterInfo); 대신 ctx.address().do_send(FetchClusterInfo);
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
    for (context_name, client) in &kube_contexts.contexts {
        info!(">>> [Context: {}] 클러스터 정보 조회 시작...", context_name);
        let nodes_api: Api<Node> = Api::all(client.clone());
        let pods_api: Api<Pod> = Api::all(client.clone());
        let lp = ListParams::default();

        let nodes_res = nodes_api.list(&lp).await;
        let pods_res = pods_api.list(&lp).await;

        if let (Ok(nodes), Ok(pods)) = (nodes_res, pods_res) {
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
                let gpu_count = node_labels.get("nvidia.com/gpu.count").cloned().unwrap_or_else(|| "0".to_string());
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

            // 가공된 클러스터 정보를 웹소켓 액터에게 메시지로 전송
            addr.do_send(cluster_info);
            info!("✅ [Context: {}] 클러스터 정보를 클라이언트로 전송했습니다.", context_name);
        } else {
            warn!("⚠️ [Context: {}] 정보 조회에 실패하여 건너뜁니다. (노드 또는 파드 조회 오류)", context_name);
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
    if let Ok(config) = Kubeconfig::read() {
        for context in &config.contexts {
            let context_name = &context.name;
            let options = KubeConfigOptions { context: Some(context_name.clone()), ..Default::default() };
            if let Ok(config_for_context) = Config::from_custom_kubeconfig(config.clone(), &options).await {
                if let Ok(client) = Client::try_from(config_for_context) {
                    contexts.insert(context_name.clone(), client);
                } else {
                    warn!("⚠️ [Context: {}] 클라이언트 생성에 실패했습니다.", context_name);
                }
            } else {
                warn!("⚠️ [Context: {}] 설정 로드에 실패했습니다.", context_name);
            }
        }
    } else {
        warn!("⚠️ Kubeconfig 파일을 읽는 데 실패했습니다. 기본 설정으로 진행합니다.");
    }

    // --- 사전 접속 테스트 로직 ---
    info!("--- 클러스터 사전 접속 테스트 시작 ---");
    let mut successfully_connected_contexts = HashMap::new();
    for (context_name, client) in contexts.drain() {
        let nodes_api: Api<Node> = Api::all(client.clone());
        let lp = ListParams::default().limit(1);

        match nodes_api.list(&lp).await {
            Ok(_) => {
                info!("✅ [Context: {}] Kubernetes API 서버에 성공적으로 접속했습니다.", context_name);
                successfully_connected_contexts.insert(context_name, client);
            },
            Err(e) => {
                error!("❌ [Context: {}] Kubernetes API 서버 접속 테스트 실패: {}", context_name, e);
            }
        }
    }
    info!("--- 클러스터 사전 접속 테스트 완료 ---");

    // 성공적으로 접속된 클라이언트만 사용하여 KubeContexts 생성
    let kube_contexts = web::Data::new(Arc::new(KubeContexts { contexts: successfully_connected_contexts }));

    // 만약 접속 가능한 클러스터가 하나도 없다면 서버를 시작하지 않거나 경고
    if kube_contexts.contexts.is_empty() {
        error!("🚨 접속 가능한 Kubernetes 클러스터가 없습니다. 서버를 시작하지 않습니다.");
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
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}

