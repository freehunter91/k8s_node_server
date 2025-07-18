// main.rs
use actix::Actor;
use actix_files as fs;
use actix_web::{web, App, HttpServer};
use futures::future::join_all;
use kube::{Client, Config, config::{KubeConfigOptions, Kubeconfig}};
use std::collections::HashMap;
use log::{error, info, warn};

mod db;
mod handlers;
mod models;
mod dummy_data;

use handlers::{ws_index, WsServer};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    let kubeconfig = match Kubeconfig::read() {
        Ok(conf) => conf,
        Err(e) => { 
            warn!("Kubeconfig 파일을 읽는 데 실패했습니다: {}. 더미 데이터 모드로 시작합니다.", e);
            let server = WsServer::new_with_dummy_data();
            let server_addr = server.start();
            
            info!("웹 서버를 http://127.0.0.1:8080 에서 시작합니다 (더미 데이터 모드).");
            
            HttpServer::new(move || {
                App::new()
                    .app_data(web::Data::new(server_addr.clone()))
                    .route("/ws/", web::get().to(ws_index))
                    .service(fs::Files::new("/", "./static").index_file("index.html"))
            })
            .bind("127.0.0.1:8080")?
            .run()
            .await?;
            
            return Ok(());
        }
    };

    let mut client_creation_futures = vec![];
    for context in &kubeconfig.contexts {
        let context_name = context.name.clone();
        let kubeconfig_clone = kubeconfig.clone();
        client_creation_futures.push(tokio::spawn(async move {
            info!("[{}] 클라이언트 생성을 시도합니다...", context_name);
            let options = KubeConfigOptions { context: Some(context_name.clone()), ..Default::default() };
            let config_result = Config::from_custom_kubeconfig(kubeconfig_clone, &options).await;
            let mut client_config = match config_result {
                Ok(c) => c,
                Err(e) => { error!("[{}] 설정 로드 실패: {}", context_name, e); return None; }
            };
            client_config.auth_info.token = Some("여기에_ID_토큰을_붙여넣으세요".into());
            client_config.auth_info.exec = None;
            match Client::try_from(client_config) {
                Ok(client) => { info!("[{}] 클라이언트 생성 성공", context_name); Some((context_name, client)) }
                Err(e) => { error!("[{}] 클라이언트 생성 실패: {}", context_name, e); None }
            }
        }));
    }

    let results = join_all(client_creation_futures).await;
    let mut user_clients = HashMap::new();
    for result in results {
        if let Ok(Some((name, client))) = result {
            user_clients.insert(name, client);
        }
    }

    if user_clients.is_empty() { 
        warn!("성공적으로 생성된 클라이언트가 없습니다. 더미 데이터 모드로 시작합니다.");
        let server = WsServer::new_with_dummy_data();
        let server_addr = server.start();
        
        info!("웹 서버를 http://127.0.0.1:8080 에서 시작합니다 (더미 데이터 모드).");
        
        HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(server_addr.clone()))
                .route("/ws/", web::get().to(ws_index))
                .service(fs::Files::new("/", "./static").index_file("index.html"))
        })
        .bind("127.0.0.1:8080")?
        .run()
        .await?;
        
        return Ok(());
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
