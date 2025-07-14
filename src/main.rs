use actix_cors::Cors;
use actix_files as fs;
use actix_web::{get, web, App, HttpResponse, HttpServer, Responder};
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::{
    api::ListParams,
    config::{KubeConfigOptions, Kubeconfig},
    Api, Client, Config,
};
use log::{error, info}; // log 라이브러리 사용
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

// --- 데이터 모델 (변경 없음) ---
#[derive(Serialize, Deserialize, Clone, Debug)]
struct ContainerInfo { name: String, image: String }
#[derive(Serialize, Deserialize, Clone, Debug)]
struct PodInfo { name: String, namespace: String, node_name: String, labels: HashMap<String, String>, containers: Vec<ContainerInfo>, cluster_name: String }
#[derive(Serialize, Deserialize, Clone, Debug)]
struct NodeInfo { name: String, labels: HashMap<String, String>, pods: Vec<PodInfo>, pod_count: usize, container_count: usize, cluster_name: String, os_image: String, kubelet_version: String, architecture: String, capacity_cpu: String, capacity_memory: String }
#[derive(Serialize, Deserialize, Clone, Debug)]
struct ClusterInfo { name: String, nodes: Vec<NodeInfo>, node_count: usize, pod_count: usize }

// --- API 핸들러 (오류 추적 기능 강화) ---
#[get("/clusters")]
async fn get_all_clusters_info(
    kube_contexts: web::Data<Arc<KubeContexts>>,
) -> impl Responder {
    let mut clusters_info = vec![];
    for (context_name, client) in &kube_contexts.contexts {
        info!(">>> [Context: {}] 클러스터 정보 조회 시작...", context_name);
        let nodes_api: Api<Node> = Api::all(client.clone());
        let pods_api: Api<Pod> = Api::all(client.clone());
        let lp = ListParams::default();

        // 순차적으로 호출하여 어느 부분에서 문제인지 명확히 확인
        info!("... [Context: {}] 노드 목록을 요청합니다.", context_name);
        let nodes_res = nodes_api.list(&lp).await;
        let nodes = match nodes_res {
            Ok(n) => n,
            Err(e) => {
                let error_message = format!("Error fetching nodes for context '{}': {}", context_name, e);
                error!("!!! {}", error_message); // 터미널에 에러 로그
                return HttpResponse::InternalServerError().body(error_message);
            }
        };
        info!("... [Context: {}] 노드 {}개를 성공적으로 가져왔습니다.", context_name, nodes.items.len());

        info!("... [Context: {}] 파드 목록을 요청합니다.", context_name);
        let pods_res = pods_api.list(&lp).await;
        let pods = match pods_res {
            Ok(p) => p,
            Err(e) => {
                let error_message = format!("Error fetching pods for context '{}': {}", context_name, e);
                error!("!!! {}", error_message);
                return HttpResponse::InternalServerError().body(error_message);
            }
        };
        info!("... [Context: {}] 파드 {}개를 성공적으로 가져왔습니다.", context_name, pods.items.len());

        let mut nodes_info = vec![];
        for node in nodes {
            let node_name = node.metadata.name.clone().unwrap_or_default();
            let node_pods: Vec<PodInfo> = pods.iter().filter(|p| p.spec.as_ref().and_then(|s| s.node_name.as_ref()) == Some(&node_name))
                .map(|p| {
                    let containers = p.spec.as_ref().map(|s| s.containers.iter().map(|c| ContainerInfo {
                        name: c.name.clone(),
                        image: c.image.clone().unwrap_or_default(),
                    }).collect()).unwrap_or_default();
                    PodInfo {
                        name: p.metadata.name.clone().unwrap_or_default(),
                        namespace: p.metadata.namespace.clone().unwrap_or_default(),
                        node_name: node_name.clone(),
                        labels: p.metadata.labels.clone().unwrap_or_default().into_iter().collect(),
                        containers,
                        cluster_name: context_name.clone(),
                    }
                }).collect();

            let container_count = node_pods.iter().map(|p| p.containers.len()).sum();
            let node_status = node.status.as_ref();
            let node_info_details = node_status.and_then(|s| s.node_info.as_ref());

            nodes_info.push(NodeInfo {
                name: node_name.clone(),
                labels: node.metadata.labels.clone().unwrap_or_default().into_iter().collect(),
                pod_count: node_pods.len(),
                container_count,
                pods: node_pods,
                cluster_name: context_name.clone(),
                os_image: node_info_details.map_or("N/A".to_string(), |ni| ni.os_image.clone()),
                kubelet_version: node_info_details.map_or("N/A".to_string(), |ni| ni.kubelet_version.clone()),
                architecture: node_info_details.map_or("N/A".to_string(), |ni| ni.architecture.clone()),
                capacity_cpu: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("cpu").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()),
                capacity_memory: node_status.and_then(|s| s.capacity.as_ref()).and_then(|c| c.get("memory").map(|q| q.0.clone())).unwrap_or_else(|| "N/A".to_string()),
            });
        }
        clusters_info.push(ClusterInfo {
            name: context_name.clone(),
            node_count: nodes_info.len(),
            pod_count: nodes_info.iter().map(|n| n.pod_count).sum(),
            nodes: nodes_info,
        });
        info!(">>> [Context: {}] 클러스터 정보 처리 완료.", context_name);
    }
    HttpResponse::Ok().json(clusters_info)
}

struct KubeContexts {
    contexts: HashMap<String, Client>,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // 로거 초기화. RUST_LOG 환경 변수로 로그 레벨을 제어합니다.
    // (예: RUST_LOG=info cargo run)
    env_logger::init();

    info!("K8s 대시보드 백엔드 서버 초기화 중...");
    
    let mut contexts = HashMap::new();
    if let Ok(config) = Kubeconfig::read() {
        for context in &config.contexts {
            let context_name = &context.name;
            info!("... '{}' 컨텍스트 설정 읽는 중...", context_name);
            let options = KubeConfigOptions { context: Some(context_name.clone()), ..Default::default() };
            if let Ok(config_for_context) = Config::from_custom_kubeconfig(config.clone(), &options).await {
                if let Ok(client) = Client::try_from(config_for_context) {
                    info!("✅ '{}' 클러스터 클라이언트 생성 성공", context_name);
                    contexts.insert(context_name.clone(), client);
                } else {
                    error!("⚠️ '{}' 클러스터 클라이언트 생성 실패", context_name);
                }
            }
        }
    } else {
        error!("⚠️ kubeconfig 파일을 찾을 수 없습니다.");
    }
    
    let kube_contexts = web::Data::new(Arc::new(KubeContexts { contexts }));

    info!("\n🚀 서버 시작: http://127.0.0.1:8080");

    HttpServer::new(move || {
        let cors = Cors::default().allow_any_origin().allowed_methods(vec!["GET"]).allow_any_header().max_age(3600);

        App::new()
            .wrap(cors)
            .app_data(kube_contexts.clone()) // app_data를 공유합니다.
            .service(
                web::scope("/api")
                    .service(get_all_clusters_info),
            )
            .service(fs::Files::new("/", "./static").index_file("index.html"))
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
