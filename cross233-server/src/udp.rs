use crate::service::SharedServiceState;
use cross233_protocol::Message;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;

struct UdpService {
    socket: Arc<UdpSocket>,
    clients: RwLock<HashMap<SocketAddr, ()>>,
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
        let svc = Arc::new(UdpService {
            socket,
            clients: RwLock::new(HashMap::new()),
        });
        {
            let mut services = self.services.write().await;
            services.insert(service_name.to_string(), svc.clone());
        }
        let this = self.clone();
        let name = service_name.to_string();
        tokio::spawn(async move {
            let _ = this.run_udp_service(name, svc).await;
        });
        Ok(())
    }

    pub async fn stop_service(&self, service_name: &str) {
        self.services.write().await.remove(service_name);
    }

    async fn run_udp_service(
        self: Arc<Self>,
        service_name: String,
        svc: Arc<UdpService>,
    ) -> anyhow::Result<()> {
        let mut buf = vec![0u8; 65536];
        loop {
            let (n, from) = match svc.socket.recv_from(&mut buf).await {
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
        self.services.write().await.remove(&service_name);
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
