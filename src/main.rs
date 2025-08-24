mod config;
mod log_analyzer;
use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use config::CONFIG;
use log_analyzer::LogAnalyzer;
mod tlds;
mod tui;

#[tokio::main]
async fn main() -> Result<()> {
    tui::test().await
    // env_logger::init();
    // let analyzer = LogAnalyzer::new().await?;
    // let _ = analyzer.analyze_loop().await;
    //
    // let app = Router::new()
    //     .route("/", get(root_get))
    //     .route("/ws/v1/get_updates", get(get_updates))
    //     .with_state(analyzer);
    //
    // log::info!(
    //     "Starting Webserver on port: {}:{}",
    //     CONFIG.server_addr, CONFIG.server_port
    // );
    // let listener =
    //     tokio::net::TcpListener::bind(format!("{}:{}", CONFIG.server_addr, CONFIG.server_port))
    //         .await?;
    // axum::serve(listener, app).await?;

    Ok(())
}

async fn root_get() -> impl IntoResponse {
    let markup = match tokio::fs::read_to_string("src/index.html").await {
        Ok(m) => m,
        Err(err) => {
            log::error!("{}",err);
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response()
        }
    };
    
    Html(markup).into_response()
}

async fn get_updates(
    ws: WebSocketUpgrade,
    State(analyzer): State<LogAnalyzer>,
) -> impl IntoResponse {
    ws.on_upgrade(|ws: WebSocket| async {
        match search_stream(analyzer, ws).await {
            Ok(_) => (),
            Err(err) => {
                log::error!("{}", err);
                return;
            }
        }
    })
}

async fn search_stream(mut analyzer: LogAnalyzer, mut ws: WebSocket) -> Result<()> {
    log::debug!("New websocket client connected!");
    loop {
        let value = analyzer.get_update().await?;
        ws.send(Message::Text(value.into())).await?;
        log::debug!("Sending update!");
    }
}
