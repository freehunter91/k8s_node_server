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
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use log::{error, info, warn};

// --- 데이터 모델 (변경 없음) ---
#[derive(Serialize, Deserialize, Clone, Debug, Message)]
#[rtype(result = "()")]
pub struct ClusterInfo { /* ... */ }
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NodeInfo { /* ... */ }
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PodInfo { /* ... */ }
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContainerInfo { /* ... */ }
// (구조체 내용은 이전과 동일하므로 생략)


// --- [신규] Keycloak 토큰 갱신 로직 ---
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

    // Keycloak 토큰 엔드포인트 URL 구성
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


// --- 액터 및 핸들러 (이전과 거의 동일) ---
#[derive(Message)]
#[rtype(result = "()")]
struct FetchData(pub Addr<WsSession>);

pub struct WsSession {
    pub server_addr: Addr<WsServer>,
}
// ... (WsSession의 Actor, Handler<ClusterInfo>, StreamHandler 구현은 이전과 동일)

pub struct WsServer {
    pub user_clients: Arc<Mutex<HashMap<String, Client>>>,
}
// ... (WsServer의 Handler<FetchData>, Actor 구현은 이전과 동일)


// --- 메인 함수 수정 ---
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

    // [수정] 모든 컨텍스트를 순회하며 토큰 갱신 및 클라이언트 생성 시도
    for context in &kubeconfig.contexts {
        let context_name = &context.name;
        let user_name = &context.context.user;

        // 컨텍스트에 해당하는 사용자 정보(auth_info) 찾기
        if let Some(named_auth) = kubeconfig.auth_infos.iter().find(|&u| &u.name == user_name) {
            // OIDC 인증 제공자 정보가 있는지 확인
            if let Some(auth_provider) = &named_auth.auth_info.auth_provider {
                
                let user_clients_clone = user_clients.clone();
                let kubeconfig_clone = kubeconfig.clone();
                let context_name_clone = context_name.clone();
                let auth_provider_clone = auth_provider.clone();

                tokio::spawn(async move {
                    info!("[{}] OIDC 토큰 갱신을 시도합니다...", context_name_clone);
                    
                    // 1. Keycloak에 접속하여 새 ID 토큰을 발급받습니다.
                    match refresh_oidc_token(&auth_provider_clone).await {
                        Ok(new_id_token) => {
                            info!("[{}] OIDC 토큰 갱신 성공.", context_name_clone);
                            
                            // 2. 새 토큰으로 kube-rs 클라이언트를 생성합니다.
                            let options = kube::config::KubeConfigOptions { context: Some(context_name_clone.clone()), ..Default::default() };
                            match Config::from_custom_kubeconfig(kubeconfig_clone, &options).await {
                                Ok(mut config) => {
                                    config.auth_info.token = Some(new_id_token.into());
                                    config.auth_info.exec = None;
                                    match Client::try_from(config) {
                                        Ok(client) => {
                                            user_clients_clone.lock().unwrap().insert(context_name_clone, client);
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

// --- 기타 함수들 (ws_index, WsServer/WsSession 구현 등) ---
// (이전 코드와 동일하므로 생략)

