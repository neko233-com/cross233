use crate::service::SharedServiceState;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;

pub async fn run_health_monitor(state: Arc<SharedServiceState>) {
    let mut tick = interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        let services = state.get_all_services().await;
        let now = std::time::Instant::now();
        for entry in services {
            if !entry.enabled.load(Ordering::Relaxed) {
                continue;
            }
            // Only services that explicitly request an active health check send
            // health reports from the client. In particular, a built-in static
            // directory has no local socket to probe and must remain available.
            if entry.config.health_check.is_none() {
                continue;
            }
            let timeout = if entry.config.health_timeout > 0 {
                Duration::from_secs(entry.config.health_timeout.max(30) as u64)
            } else {
                Duration::from_secs(90)
            };
            if now.duration_since(entry.last_health_check()) > timeout {
                tracing::warn!(service = %entry.config.name, "marking service unhealthy due to timeout");
                entry.healthy.store(false, Ordering::Relaxed);
            }
        }
    }
}
