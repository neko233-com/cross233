use crate::config::ServerConfig;
use crate::metrics::MetricsCollector;
use crate::service::SharedServiceState;
use crate::web::assets::static_handler;
use axum::{
    extract::{
        ws::{Message as WsMsg, WebSocket, WebSocketUpgrade},
        Path, Query, Request, State,
    },
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use cookie::{Cookie, SameSite};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

#[derive(Debug, Clone)]
pub struct WebState {
    pub service_state: Arc<SharedServiceState>,
    pub config: Arc<RwLock<ServerConfig>>,
    pub sessions: Arc<RwLock<HashMap<String, std::time::Instant>>>,
    pub logs: Arc<RwLock<VecDeque<LogEntry>>>,
    pub metrics: Arc<MetricsCollector>,
    pub event_tx: broadcast::Sender<ServerEvent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerEvent {
    ServiceUpdate(Vec<ServiceInfo>),
    Stats(StatsSnapshot),
    Log(LogEntry),
}

#[derive(Debug, Clone, Serialize)]
pub struct StatsSnapshot {
    pub total_services: usize,
    pub total_tx: u64,
    pub total_rx: u64,
    pub total_conns: usize,
    pub total_clients: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: u64,
    pub level: String,
    pub message: String,
}

impl WebState {
    pub fn new(
        service_state: Arc<SharedServiceState>,
        config: ServerConfig,
        metrics: Arc<MetricsCollector>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            service_state,
            config: Arc::new(RwLock::new(config)),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            logs: Arc::new(RwLock::new(VecDeque::with_capacity(500))),
            metrics,
            event_tx,
        }
    }

    pub async fn add_log(&self, level: &str, message: &str) {
        let entry = LogEntry {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            level: level.to_string(),
            message: message.to_string(),
        };
        {
            let mut logs = self.logs.write().await;
            if logs.len() >= 500 {
                logs.pop_front();
            }
            logs.push_back(entry.clone());
        }
        let _ = self.event_tx.send(ServerEvent::Log(entry));
    }

    pub async fn broadcast_services(&self) {
        let services = self.collect_services().await;
        let _ = self.event_tx.send(ServerEvent::ServiceUpdate(services));
    }

    pub async fn broadcast_stats(&self) {
        let stats = self.collect_stats().await;
        let _ = self.event_tx.send(ServerEvent::Stats(stats));
    }

    async fn collect_services(&self) -> Vec<ServiceInfo> {
        let services = self.service_state.get_all_services().await;
        services
            .into_iter()
            .map(|e| ServiceInfo {
                name: e.config.name.clone(),
                ty: e.config.effective_type().to_string(),
                local_addr: e.config.local_addr.clone(),
                remote_port: e.config.remote_port,
                host: e.config.host.clone(),
                subdomain: e.config.subdomain.clone(),
                healthy: e.healthy.load(Ordering::Relaxed),
                enabled: e.enabled.load(Ordering::Relaxed),
                current_conns: e.current_conns.load(Ordering::Relaxed),
                traffic_tx: e.traffic_tx.load(Ordering::Relaxed),
                traffic_rx: e.traffic_rx.load(Ordering::Relaxed),
                client_id: e.client_id.clone(),
                uptime_secs: e.started_at.elapsed().as_secs(),
                group: e.config.group.clone(),
                bandwidth_limit_kbps: e.config.bandwidth_limit_kbps,
            })
            .collect()
    }

    async fn collect_stats(&self) -> StatsSnapshot {
        let services = self.service_state.get_all_services().await;
        let clients = self.service_state.get_clients().await;
        let mut total_tx = 0u64;
        let mut total_rx = 0u64;
        let mut total_conns = 0usize;
        for e in &services {
            total_tx += e.traffic_tx.load(Ordering::Relaxed);
            total_rx += e.traffic_rx.load(Ordering::Relaxed);
            total_conns += e.current_conns.load(Ordering::Relaxed);
        }
        StatsSnapshot {
            total_services: services.len(),
            total_tx,
            total_rx,
            total_conns,
            total_clients: clients.len(),
        }
    }

    pub async fn cleanup_sessions(&self) {
        let mut sessions = self.sessions.write().await;
        let now = std::time::Instant::now();
        sessions.retain(|_, created| {
            now.duration_since(*created) < std::time::Duration::from_secs(24 * 3600)
        });
    }

    pub fn start_broadcast_loop(self: Arc<Self>) {
        let state = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                interval.tick().await;
                state.broadcast_services().await;
                state.broadcast_stats().await;
            }
        });
    }
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    user: String,
    password: String,
}

async fn login(State(state): State<Arc<WebState>>, Json(body): Json<LoginRequest>) -> Response {
    let config = state.config.read().await;
    let valid_user = config.web_user.clone();
    let valid_pass = config.web_password.clone();
    drop(config);

    if !valid_user.is_empty() && (body.user != valid_user || body.password != valid_pass) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid credentials"})),
        )
            .into_response();
    }

    let token = cross233_protocol::random_id();
    {
        let mut sessions = state.sessions.write().await;
        sessions.insert(token.clone(), std::time::Instant::now());
    }

    let cookie = Cookie::build(("session", token.as_str()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .max_age(cookie::time::Duration::hours(24))
        .build();

    let mut resp = Json(serde_json::json!({"ok": true})).into_response();
    if let Ok(v) = axum::http::HeaderValue::from_str(&cookie.to_string()) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

fn check_api_token(req: &Request, api_token: &str) -> bool {
    if api_token.is_empty() {
        return false;
    }
    if let Some(auth) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = auth.strip_prefix("Bearer ") {
            return token == api_token;
        }
    }
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some(rest) = pair.strip_prefix("token=") {
                return rest == api_token;
            }
        }
    }
    false
}

async fn auth_middleware(
    State(state): State<Arc<WebState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let cfg = state.config.read().await.clone();
    let needs_auth = !cfg.web_user.is_empty();
    let api_token = cfg.api_token.clone();
    let config = Arc::new(cfg);

    if !api_token.is_empty() && check_api_token(&req, &api_token) {
        req.extensions_mut().insert(config);
        return next.run(req).await;
    }

    if !needs_auth {
        req.extensions_mut().insert(config);
        return next.run(req).await;
    }

    let cookie_header = req
        .headers()
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .unwrap_or("");

    let mut session_token = None;
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("session=") {
            session_token = Some(val.to_string());
        }
    }

    if let Some(token) = session_token {
        let sessions = state.sessions.read().await;
        if let Some(created) = sessions.get(&token) {
            if created.elapsed() < std::time::Duration::from_secs(24 * 3600) {
                drop(sessions);
                req.extensions_mut().insert(config);
                return next.run(req).await;
            }
        }
    }

    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error": "authentication required"})),
    )
        .into_response()
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    name: String,
    ty: String,
    local_addr: String,
    remote_port: u16,
    host: Option<String>,
    subdomain: Option<String>,
    healthy: bool,
    enabled: bool,
    current_conns: usize,
    traffic_tx: u64,
    traffic_rx: u64,
    client_id: String,
    uptime_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    #[serde(rename = "bandwidthLimit", skip_serializing_if = "is_zero_u64")]
    bandwidth_limit_kbps: u64,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

async fn get_services(State(state): State<Arc<WebState>>) -> Json<serde_json::Value> {
    let infos = state.collect_services().await;
    Json(serde_json::json!({"services": infos}))
}

async fn get_service_detail(
    State(state): State<Arc<WebState>>,
    Path(name): Path<String>,
) -> Response {
    if let Some(entry) = state.service_state.get_service_by_name(&name).await {
        let info = ServiceInfo {
            name: entry.config.name.clone(),
            ty: entry.config.effective_type().to_string(),
            local_addr: entry.config.local_addr.clone(),
            remote_port: entry.config.remote_port,
            host: entry.config.host.clone(),
            subdomain: entry.config.subdomain.clone(),
            healthy: entry.healthy.load(Ordering::Relaxed),
            enabled: entry.enabled.load(Ordering::Relaxed),
            current_conns: entry.current_conns.load(Ordering::Relaxed),
            traffic_tx: entry.traffic_tx.load(Ordering::Relaxed),
            traffic_rx: entry.traffic_rx.load(Ordering::Relaxed),
            client_id: entry.client_id.clone(),
            uptime_secs: entry.started_at.elapsed().as_secs(),
            group: entry.config.group.clone(),
            bandwidth_limit_kbps: entry.config.bandwidth_limit_kbps,
        };
        Json(serde_json::json!({"service": info})).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found"})),
        )
            .into_response()
    }
}

async fn get_service_metrics(
    State(state): State<Arc<WebState>>,
    Path(name): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let limit = q
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(120usize);
    let history = state.metrics.get_service_history(&name, Some(limit)).await;
    Json(serde_json::json!({"metrics": history, "service": name}))
}

async fn toggle_service(
    State(state): State<Arc<WebState>>,
    Path(name): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let enabled = body
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let ok = state.service_state.toggle_service(&name, enabled).await;
    state.broadcast_services().await;
    Json(serde_json::json!({"ok": ok}))
}

async fn kick_client(
    State(state): State<Arc<WebState>>,
    Path(client_id): Path<String>,
) -> Json<serde_json::Value> {
    let tx_map = state.service_state.client_control_tx.read().await;
    if let Some(tx) = tx_map.get(&client_id) {
        let close_msg = cross233_protocol::Message::new_close("kicked by admin");
        let _ = tx.send(close_msg);
    }
    drop(tx_map);
    state.service_state.unregister_client(&client_id).await;
    state
        .add_log("warn", &format!("client {} kicked by admin", client_id))
        .await;
    state.broadcast_services().await;
    Json(serde_json::json!({"ok": true}))
}

async fn get_stats(State(state): State<Arc<WebState>>) -> Json<serde_json::Value> {
    let stats = state.collect_stats().await;
    Json(serde_json::to_value(stats).unwrap_or_default())
}

async fn get_metrics_history(
    State(state): State<Arc<WebState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let limit = q
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(120usize);
    let history = state.metrics.get_history(Some(limit)).await;
    Json(serde_json::json!({"metrics": history}))
}

async fn get_clients(State(state): State<Arc<WebState>>) -> Json<serde_json::Value> {
    let clients = state.service_state.get_clients().await;
    let mut details = Vec::new();
    let services_all = state.service_state.get_all_services().await;
    let services_by_client: HashMap<String, Vec<&crate::service::ServiceEntry>> = {
        let mut m: HashMap<String, Vec<&crate::service::ServiceEntry>> = HashMap::new();
        for s in &services_all {
            m.entry(s.client_id.clone()).or_default().push(s);
        }
        m
    };
    for (cid, svc_names) in &clients {
        let svcs: Vec<serde_json::Value> = services_by_client
            .get(cid)
            .map(|list| {
                list.iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.config.name,
                            "type": s.config.effective_type(),
                            "remote_port": s.config.remote_port,
                            "conns": s.current_conns.load(Ordering::Relaxed),
                            "healthy": s.healthy.load(Ordering::Relaxed),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        details.push(serde_json::json!({
            "id": cid,
            "services": svc_names,
            "service_count": svc_names.len(),
            "active_conns": services_by_client.get(cid).map(|list| list.iter().map(|s| s.current_conns.load(Ordering::Relaxed)).sum::<usize>()).unwrap_or(0),
            "services_detail": svcs,
        }));
    }
    Json(serde_json::json!({"clients": details}))
}

async fn get_logs(State(state): State<Arc<WebState>>) -> Json<serde_json::Value> {
    state.cleanup_sessions().await;
    let logs = state.logs.read().await;
    let entries: Vec<LogEntry> = logs.iter().cloned().collect();
    Json(serde_json::json!({"logs": entries}))
}

async fn get_config(State(state): State<Arc<WebState>>) -> Json<serde_json::Value> {
    let config = state.config.read().await;
    let mut v = serde_json::to_value(&*config).unwrap_or(serde_json::json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.remove("auth_key");
        obj.remove("api_token");
    }
    Json(v)
}

async fn reload_config(State(_state): State<Arc<WebState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"ok": true}))
}

async fn healthz() -> &'static str {
    "ok"
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<WebState>>) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: Arc<WebState>) {
    let mut rx = state.event_tx.subscribe();
    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(event) => {
                        if let Ok(json) = serde_json::to_string(&event) {
                            if socket.send(WsMsg::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            ws_msg = socket.recv() => {
                match ws_msg {
                    Some(Ok(WsMsg::Close(_))) | None => break,
                    Some(Ok(WsMsg::Ping(p))) => { let _ = socket.send(WsMsg::Pong(p)).await; }
                    _ => {}
                }
            }
        }
    }
}

async fn api_status(State(state): State<Arc<WebState>>) -> Json<serde_json::Value> {
    let stats = state.collect_stats().await;
    Json(serde_json::json!({
        "server": "cross233",
        "version": env!("CARGO_PKG_VERSION"),
        "status": "running",
        "stats": stats,
    }))
}

pub fn build_router(web_state: Arc<WebState>) -> Router {
    web_state.clone().start_broadcast_loop();

    let protected = Router::new()
        .route("/api/services", get(get_services))
        .route("/api/services/{name}", get(get_service_detail))
        .route("/api/services/{name}/metrics", get(get_service_metrics))
        .route("/api/services/{name}/toggle", post(toggle_service))
        .route("/api/clients/{id}/kick", post(kick_client))
        .route("/api/stats", get(get_stats))
        .route("/api/metrics", get(get_metrics_history))
        .route("/api/clients", get(get_clients))
        .route("/api/logs", get(get_logs))
        .route("/api/config", get(get_config))
        .route("/api/config/reload", post(reload_config))
        .route("/api/ws", get(ws_handler))
        .route("/api/v1/status", get(api_status))
        .route("/api/v1/services", get(get_services))
        .route("/api/v1/services/{name}", get(get_service_detail))
        .route("/api/v1/clients", get(get_clients))
        .route("/api/v1/logs", get(get_logs))
        .route("/api/v1/metrics", get(get_metrics_history))
        .layer(middleware::from_fn_with_state(
            web_state.clone(),
            auth_middleware,
        ));

    let public = Router::new()
        .route("/api/login", post(login))
        .route("/healthz", get(healthz));

    Router::new()
        .merge(public)
        .merge(protected)
        .fallback(static_handler)
        .with_state(web_state)
}
