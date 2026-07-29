use crate::service::SharedServiceState;
use cross233_protocol::Message;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{watch, RwLock};

struct UdpService {
    socket: Arc<UdpSocket>,
    clients: RwLock<HashMap<SocketAddr, ()>>,
    shutdown: watch::Sender<bool>,
}

pub struct UdpManager {
    services: Arc<RwLock<HashMap<String, Arc<UdpService>>>>,
    state: Arc<SharedServiceState>,
    bind: String,
}

impl UdpManager {
    pub fn new(state: Arc<SharedServiceState>, bind: String) -> Arc<Self> {
        Arc::new(Self {
            services: Arc::new(RwLock::new(HashMap::new())),
            state,
            bind,
        })
    }

    pub async fn start_service(
        self: &Arc<Self>,
        service_name: &str,
        port: u16,
    ) -> anyhow::Result<()> {
        {
            let services = self.services.read().await;
            if services.contains_key(service_name) {
                return Ok(());
            }
        }
        let addr = format!("{}:{}", self.bind, port);
        let socket = Arc::new(UdpSocket::bind(&addr).await?);
        tracing::info!(service = %service_name, addr = %addr, "UDP listener started");
        let (shutdown, shutdown_rx) = watch::channel(false);
        let svc = Arc::new(UdpService {
            socket,
            clients: RwLock::new(HashMap::new()),
            shutdown,
        });
        {
            let mut services = self.services.write().await;
            services.insert(service_name.to_string(), svc.clone());
        }
        let this = self.clone();
        let name = service_name.to_string();
        tokio::spawn(async move {
            let _ = this.run_udp_service(name, svc, shutdown_rx).await;
        });
        Ok(())
    }

    pub async fn stop_service(&self, service_name: &str) {
        if let Some(service) = self.services.write().await.remove(service_name) {
            let _ = service.shutdown.send(true);
            tracing::info!(service = %service_name, "UDP listener stopped");
        }
    }

    async fn run_udp_service(
        self: Arc<Self>,
        service_name: String,
        svc: Arc<UdpService>,
        mut shutdown: watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let mut buf = vec![0u8; 65536];
        loop {
            let received = tokio::select! {
                _ = shutdown.changed() => break,
                received = svc.socket.recv_from(&mut buf) => received,
            };
            let (n, from) = match received {
                Ok(x) => x,
                Err(e) => {
                    tracing::debug!(service = %service_name, "udp recv error: {}", e);
                    break;
                }
            };
            let entry = match self.state.get_service_by_name(&service_name).await {
                Some(e)
                    if e.enabled.load(Ordering::Relaxed) && e.healthy.load(Ordering::Relaxed) =>
                {
                    e
                }
                _ => continue,
            };
            {
                let mut clients = svc.clients.write().await;
                clients.insert(from, ());
            }
            let msg = Message::new_udp(
                entry.config.remote_port,
                &from.to_string(),
                buf[..n].to_vec(),
                entry.config.clone(),
            );
            let _ = entry.control_tx.send(msg);
        }
        let mut services = self.services.write().await;
        if services
            .get(&service_name)
            .is_some_and(|current| Arc::ptr_eq(current, &svc))
        {
            services.remove(&service_name);
        }
        Ok(())
    }

    pub async fn forward_response(&self, service_name: &str, addr_str: &str, data: &[u8]) {
        if let Ok(addr) = addr_str.parse::<SocketAddr>() {
            if let Some(svc) = self.services.read().await.get(service_name) {
                let _ = svc.socket.send_to(data, addr).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UdpManager;
    use crate::service::SharedServiceState;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn stopping_udp_service_releases_its_port() {
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let state = SharedServiceState::new(60080, 60090, 0);
        let manager = UdpManager::new(state, "127.0.0.1".to_string());
        manager
            .start_service("udp-release-test", port)
            .await
            .unwrap();
        assert!(std::net::UdpSocket::bind(("127.0.0.1", port)).is_err());

        manager.stop_service("udp-release-test").await;
        timeout(Duration::from_secs(1), async {
            loop {
                if std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("UDP listener should release promptly");
    }
}
