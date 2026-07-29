use cross233_protocol::Message;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

pub struct HealthCheckConfig {
    pub type_: String,
    pub interval: u32,
    pub timeout_ms: u32,
    pub max_failed: u32,
    pub path: Option<String>,
    pub headers: std::collections::HashMap<String, String>,
}

impl HealthCheckConfig {
    pub fn from_service(svc: &cross233_protocol::Service) -> Option<Self> {
        let hc_type = svc.health_check.as_deref()?;
        if hc_type.is_empty() {
            return None;
        }
        Some(Self {
            type_: hc_type.to_string(),
            interval: if svc.health_interval > 0 {
                svc.health_interval
            } else {
                10
            },
            timeout_ms: if svc.health_timeout > 0 {
                svc.health_timeout
            } else {
                3
            },
            max_failed: if svc.health_max_failed > 0 {
                svc.health_max_failed
            } else {
                1
            },
            path: svc.health_path.clone(),
            headers: svc.health_headers.clone(),
        })
    }
}

pub async fn run_health_check(
    service_name: String,
    local_addr: String,
    config: HealthCheckConfig,
    out_tx: mpsc::UnboundedSender<Message>,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    let mut fail_count = 0u32;
    let interval = Duration::from_secs(config.interval as u64);
    let timeout_dur = Duration::from_secs(config.timeout_ms as u64);
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => break,
            _ = ticker.tick() => {
                let healthy = check_health(&local_addr, &config, timeout_dur).await;
                if healthy {
                    fail_count = 0;
                } else {
                    fail_count += 1;
                }
                let reported = healthy || fail_count >= config.max_failed;
                if reported {
                    let _ = out_tx.send(Message::new_health(&service_name, healthy));
                }
            }
        }
    }
}

async fn check_health(addr: &str, config: &HealthCheckConfig, timeout_dur: Duration) -> bool {
    match config.type_.as_str() {
        "http" | "https" => check_http(addr, config, timeout_dur).await,
        _ => check_tcp(addr, timeout_dur).await,
    }
}

async fn check_tcp(addr: &str, timeout_dur: Duration) -> bool {
    timeout(timeout_dur, TcpStream::connect(addr))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

async fn check_http(addr: &str, config: &HealthCheckConfig, timeout_dur: Duration) -> bool {
    let path = config.path.as_deref().unwrap_or("/");
    let url = if addr.starts_with("http://") || addr.starts_with("https://") {
        format!("{addr}{path}")
    } else {
        format!("http://{addr}{path}")
    };
    let _ = &url;
    check_tcp(addr, timeout_dur).await
}
