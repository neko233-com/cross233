use axum::{extract::State, routing::get, Json, Router};
use cross233_protocol::Service;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Default, Serialize)]
pub struct WebStateData {
    pub connected: bool,
    pub server_addr: String,
    pub client_id: String,
    pub services: Vec<Service>,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub tunnels_active: u32,
    pub qcp_port: u16,
    pub qcp_tunnel_port: u16,
    pub transport: String,
    pub active_tunnel_transport: String,
    pub transport_cache_file: String,
}

pub type WebState = Arc<RwLock<WebStateData>>;

pub fn new_state() -> WebState {
    Arc::new(RwLock::new(WebStateData::default()))
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    connected: bool,
    server_addr: String,
    client_id: String,
    services: Vec<Service>,
    bytes_in: u64,
    bytes_out: u64,
    tunnels_active: u32,
    qcp_port: u16,
    qcp_tunnel_port: u16,
    transport: String,
    active_tunnel_transport: String,
    transport_cache_file: String,
}

async fn get_status(State(state): State<WebState>) -> Json<StatusResponse> {
    let s = state.read().await;
    Json(StatusResponse {
        connected: s.connected,
        server_addr: s.server_addr.clone(),
        client_id: s.client_id.clone(),
        services: s.services.clone(),
        bytes_in: s.bytes_in,
        bytes_out: s.bytes_out,
        tunnels_active: s.tunnels_active,
        qcp_port: s.qcp_port,
        qcp_tunnel_port: s.qcp_tunnel_port,
        transport: s.transport.clone(),
        active_tunnel_transport: s.active_tunnel_transport.clone(),
        transport_cache_file: s.transport_cache_file.clone(),
    })
}

#[derive(Debug, Serialize)]
struct ConfigResponse {
    server_addr: String,
    server_name: String,
    client_id: String,
    insecure: bool,
    max_tunnels: u32,
    web_addr: String,
    qcp_port: u16,
    qcp_tunnel_port: u16,
    transport: String,
    transport_cache_file: String,
    services: Vec<Service>,
}

async fn get_config(State(state): State<WebState>) -> Json<ConfigResponse> {
    let s = state.read().await;
    Json(ConfigResponse {
        server_addr: s.server_addr.clone(),
        server_name: "cross233".to_string(),
        client_id: s.client_id.clone(),
        insecure: false,
        max_tunnels: 64,
        web_addr: "127.0.0.1:7721".to_string(),
        qcp_port: s.qcp_port,
        qcp_tunnel_port: s.qcp_tunnel_port,
        transport: s.transport.clone(),
        transport_cache_file: s.transport_cache_file.clone(),
        services: s.services.clone(),
    })
}

pub async fn start_web_server(addr: &str, state: WebState) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/api/status", get(get_status))
        .route("/api/config", get(get_config))
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(web_addr = %addr, "admin web server listening");
    axum::serve(listener, app).await?;
    Ok(())
}
