pub mod api;
pub mod assets;

use crate::config::ServerConfig;
use crate::metrics::MetricsCollector;
use crate::service::SharedServiceState;
use api::{build_router, WebState};
use std::sync::Arc;

pub fn create_web_app(
    service_state: Arc<SharedServiceState>,
    config: ServerConfig,
    metrics: Arc<MetricsCollector>,
) -> axum::Router {
    let web_state = Arc::new(WebState::new(service_state, config, metrics));
    build_router(web_state)
}

pub async fn run_web_server(
    service_state: Arc<SharedServiceState>,
    config: ServerConfig,
    addr: &str,
    metrics: Arc<MetricsCollector>,
) -> anyhow::Result<()> {
    let app = create_web_app(service_state, config, metrics);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(addr = %addr, "web server listening");
    axum::serve(listener, app).await?;
    Ok(())
}
