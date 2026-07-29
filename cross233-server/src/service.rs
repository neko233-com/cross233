use cross233_protocol::{random_id, Message, Service};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::time::{timeout, Duration};

/// Maximum time an inbound connection waits for the client to attach its
/// authenticated reverse tunnel.  This leaves room for the client's bounded
/// TCP retry sequence when an upstream path transiently drops a TLS handshake.
pub const TUNNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct WorkReq {
    pub id: String,
    pub service_name: String,
    pub src_addr: Option<SocketAddr>,
    pub dst_addr: Option<SocketAddr>,
}

pub trait TunnelIo: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> TunnelIo for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

pub type TunnelStream = Box<dyn TunnelIo>;

pub struct ServiceEntry {
    pub config: Service,
    pub client_id: String,
    pub healthy: AtomicBool,
    pub enabled: AtomicBool,
    pub control_tx: mpsc::UnboundedSender<Message>,
    pub current_conns: AtomicUsize,
    pub traffic_tx: AtomicU64,
    pub traffic_rx: AtomicU64,
    pub started_at: Instant,
    last_health_check: std::sync::RwLock<Instant>,
    round_robin_idx: AtomicUsize,
}

impl std::fmt::Debug for ServiceEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceEntry")
            .field("name", &self.config.name)
            .field("client_id", &self.client_id)
            .finish()
    }
}

impl ServiceEntry {
    pub fn new(
        config: Service,
        client_id: String,
        control_tx: mpsc::UnboundedSender<Message>,
    ) -> Self {
        Self {
            config,
            client_id,
            healthy: AtomicBool::new(true),
            enabled: AtomicBool::new(true),
            control_tx,
            current_conns: AtomicUsize::new(0),
            traffic_tx: AtomicU64::new(0),
            traffic_rx: AtomicU64::new(0),
            started_at: Instant::now(),
            last_health_check: std::sync::RwLock::new(Instant::now()),
            round_robin_idx: AtomicUsize::new(0),
        }
    }

    pub fn last_health_check(&self) -> Instant {
        *self.last_health_check.read().unwrap()
    }
}

type PendingWorkSender = oneshot::Sender<Result<TunnelStream, String>>;

pub struct SharedServiceState {
    pub services_by_name: RwLock<HashMap<String, Arc<ServiceEntry>>>,
    pub services_by_group: RwLock<HashMap<String, Vec<Arc<ServiceEntry>>>>,
    pub client_services: RwLock<HashMap<String, Vec<String>>>,
    pub allocated_ports: RwLock<HashMap<u16, String>>,
    pub active_listeners: RwLock<HashMap<u16, ()>>,
    next_port: AtomicUsize,
    port_min: u16,
    port_max: u16,
    pub pending_work: RwLock<HashMap<String, PendingWorkSender>>,
    pub pending_tunnels_stcp: RwLock<HashMap<String, mpsc::Sender<WorkReq>>>,
    pub client_control_tx: RwLock<HashMap<String, mpsc::UnboundedSender<Message>>>,
    qcp_port: u16,
}

impl std::fmt::Debug for SharedServiceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedServiceState")
            .field("port_min", &self.port_min)
            .field("port_max", &self.port_max)
            .field("qcp_port", &self.qcp_port)
            .finish()
    }
}

impl SharedServiceState {
    pub fn new(port_min: u16, port_max: u16, qcp_port: u16) -> Arc<Self> {
        Arc::new(Self {
            services_by_name: RwLock::new(HashMap::new()),
            services_by_group: RwLock::new(HashMap::new()),
            client_services: RwLock::new(HashMap::new()),
            allocated_ports: RwLock::new(HashMap::new()),
            active_listeners: RwLock::new(HashMap::new()),
            next_port: AtomicUsize::new(port_min as usize),
            port_min,
            port_max,
            pending_work: RwLock::new(HashMap::new()),
            pending_tunnels_stcp: RwLock::new(HashMap::new()),
            client_control_tx: RwLock::new(HashMap::new()),
            qcp_port,
        })
    }

    pub async fn register_services(
        &self,
        client_id: &str,
        control_tx: mpsc::UnboundedSender<Message>,
        services: Vec<Service>,
    ) -> Vec<Service> {
        let mut assigned = Vec::new();
        {
            let mut tx = self.client_control_tx.write().await;
            tx.insert(client_id.to_string(), control_tx.clone());
        }
        let mut client_services = self.client_services.write().await;
        let svc_list = client_services.entry(client_id.to_string()).or_default();

        for mut svc in services {
            if !svc.is_enabled() {
                continue;
            }
            let name = svc.name.clone();

            if svc.uses_auto_port() {
                if let Some(port) = self.allocate_port(&name).await {
                    svc.remote_port = port;
                }
            } else if svc.is_tcp() && !svc.is_vhost() {
                let mut ports = self.allocated_ports.write().await;
                ports.entry(svc.remote_port).or_insert_with(|| name.clone());
            }

            let entry = Arc::new(ServiceEntry::new(
                svc.clone(),
                client_id.to_string(),
                control_tx.clone(),
            ));

            {
                let mut by_name = self.services_by_name.write().await;
                by_name.insert(name.clone(), entry.clone());
            }

            if let Some(group) = &svc.group {
                let mut by_group = self.services_by_group.write().await;
                by_group
                    .entry(group.clone())
                    .or_default()
                    .push(entry.clone());
            }

            if !svc_list.iter().any(|existing| existing == &name) {
                svc_list.push(name.clone());
            }
            assigned.push(svc);
        }
        assigned
    }

    pub async fn mark_listener_active(&self, port: u16) {
        self.active_listeners.write().await.insert(port, ());
    }

    pub async fn is_listener_active(&self, port: u16) -> bool {
        self.active_listeners.read().await.contains_key(&port)
    }

    async fn allocate_port(&self, name: &str) -> Option<u16> {
        let mut ports = self.allocated_ports.write().await;
        for _ in 0..(self.port_max - self.port_min + 1) {
            let idx = self.next_port.fetch_add(1, Ordering::Relaxed);
            let port = self.port_min + (idx % (self.port_max - self.port_min + 1) as usize) as u16;
            if let std::collections::hash_map::Entry::Vacant(entry) = ports.entry(port) {
                entry.insert(name.to_string());
                return Some(port);
            }
        }
        None
    }

    pub async fn is_current_control(
        &self,
        client_id: &str,
        control_tx: &mpsc::UnboundedSender<Message>,
    ) -> bool {
        self.client_control_tx
            .read()
            .await
            .get(client_id)
            .is_some_and(|current| current.same_channel(control_tx))
    }

    /// Removes a session only when its control sender is still current.
    ///
    /// A client reconnect can finish authenticating before the older control
    /// loop observes EOF. The old loop must not remove the newly registered
    /// services in that case.
    pub async fn unregister_client_if_current(
        &self,
        client_id: &str,
        control_tx: &mpsc::UnboundedSender<Message>,
    ) -> bool {
        {
            let mut controls = self.client_control_tx.write().await;
            let is_current = controls
                .get(client_id)
                .is_some_and(|current| current.same_channel(control_tx));
            if !is_current {
                return false;
            }
            controls.remove(client_id);
        }
        self.remove_client_services(client_id).await;
        true
    }

    pub async fn unregister_client(&self, client_id: &str) {
        self.client_control_tx.write().await.remove(client_id);
        self.remove_client_services(client_id).await;
    }

    async fn remove_client_services(&self, client_id: &str) {
        let svc_names: Vec<String> = {
            let mut client_services = self.client_services.write().await;
            client_services.remove(client_id).unwrap_or_default()
        };

        let mut by_name = self.services_by_name.write().await;
        let mut ports = self.allocated_ports.write().await;
        let mut by_group = self.services_by_group.write().await;

        for name in &svc_names {
            if let Some(entry) = by_name.remove(name) {
                if entry.config.remote_port != 0 && !entry.config.is_vhost() {
                    ports.remove(&entry.config.remote_port);
                }
                if let Some(group) = &entry.config.group {
                    if let Some(entries) = by_group.get_mut(group) {
                        entries.retain(|e| e.config.name != *name);
                        if entries.is_empty() {
                            by_group.remove(group);
                        }
                    }
                }
            }
        }
    }

    pub async fn get_service_by_name(&self, name: &str) -> Option<Arc<ServiceEntry>> {
        self.services_by_name.read().await.get(name).cloned()
    }

    pub async fn get_service_by_host(&self, host: &str) -> Option<Arc<ServiceEntry>> {
        let by_name = self.services_by_name.read().await;
        let host_lower = host.to_ascii_lowercase();
        for (_, entry) in by_name.iter() {
            if !entry.enabled.load(Ordering::Relaxed) {
                continue;
            }
            if let Some(h) = &entry.config.host {
                if h.eq_ignore_ascii_case(&host_lower) {
                    return Some(entry.clone());
                }
            }
            if let Some(sub) = &entry.config.subdomain {
                if host_lower.starts_with(sub) {
                    return Some(entry.clone());
                }
            }
        }
        None
    }

    pub async fn get_group_service(&self, group: &str) -> Option<Arc<ServiceEntry>> {
        let by_group = self.services_by_group.read().await;
        let entries = by_group.get(group)?;
        if entries.is_empty() {
            return None;
        }
        let idx = entries[0].round_robin_idx.fetch_add(1, Ordering::Relaxed) % entries.len();
        Some(entries[idx].clone())
    }

    pub async fn open_tunnel(
        &self,
        service: &Arc<ServiceEntry>,
        src_addr: Option<SocketAddr>,
        dst_addr: Option<SocketAddr>,
        wait: Duration,
    ) -> anyhow::Result<TunnelStream> {
        let id = random_id();
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending_work.write().await;
            pending.insert(id.clone(), tx);
        }

        let address = src_addr.map(|a| a.to_string()).unwrap_or_default();
        let local_address = dst_addr.map(|a| a.to_string()).unwrap_or_default();
        let msg = Message::new_open(&id, &address, &local_address, service.config.clone());
        if let Err(error) = service.control_tx.send(msg) {
            self.cancel_pending_tunnel(&id).await;
            return Err(anyhow::anyhow!("failed to send open: {}", error));
        }

        match timeout(wait, rx).await {
            Ok(Ok(Ok(tunnel))) => Ok(tunnel),
            Ok(Ok(Err(error))) => Err(anyhow::anyhow!("tunnel rejected: {}", error)),
            Ok(Err(_)) => Err(anyhow::anyhow!("tunnel channel closed")),
            Err(_) => {
                self.cancel_pending_tunnel(&id).await;
                Err(anyhow::anyhow!("tunnel open timeout"))
            }
        }
    }

    /// Remove a request whose inbound peer gave up before its reverse tunnel
    /// arrived.  It is safe to call after completion because removal is a
    /// no-op when the client has already claimed the request.
    pub async fn cancel_pending_tunnel(&self, id: &str) {
        self.pending_work.write().await.remove(id);
    }

    pub async fn complete_tunnel(&self, id: &str, stream: TunnelStream) -> bool {
        let mut pending = self.pending_work.write().await;
        if let Some(tx) = pending.remove(id) {
            let _ = tx.send(Ok(stream));
            true
        } else {
            false
        }
    }

    pub async fn reject_tunnel(&self, id: &str, err: &str) {
        let mut pending = self.pending_work.write().await;
        if let Some(tx) = pending.remove(id) {
            let _ = tx.send(Err(err.to_string()));
        }
    }

    pub async fn report_health(&self, service_name: &str, healthy: bool) {
        if let Some(entry) = self.services_by_name.read().await.get(service_name) {
            entry.healthy.store(healthy, Ordering::Relaxed);
            *entry.last_health_check.write().unwrap() = Instant::now();
        }
    }

    pub async fn record_traffic(&self, service_name: &str, tx: u64, rx: u64) {
        if let Some(entry) = self.services_by_name.read().await.get(service_name) {
            entry.traffic_tx.fetch_add(tx, Ordering::Relaxed);
            entry.traffic_rx.fetch_add(rx, Ordering::Relaxed);
        }
    }

    pub async fn get_all_services(&self) -> Vec<Arc<ServiceEntry>> {
        self.services_by_name
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    pub async fn get_clients(&self) -> HashMap<String, Vec<String>> {
        self.client_services.read().await.clone()
    }

    pub async fn toggle_service(&self, name: &str, enabled: bool) -> bool {
        if let Some(entry) = self.services_by_name.read().await.get(name) {
            entry.enabled.store(enabled, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn qcp_port(&self) -> u16 {
        self.qcp_port
    }

    pub async fn get_service_name_by_port(&self, port: u16) -> Option<String> {
        let by_name = self.services_by_name.read().await;
        for (_, entry) in by_name.iter() {
            if entry.config.remote_port == port {
                return Some(entry.config.name.clone());
            }
        }
        None
    }

    pub async fn remove_listener_handle(&self, port: u16) {
        self.active_listeners.write().await.remove(&port);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};

    fn test_service() -> Service {
        Service {
            name: "session-test".to_string(),
            ty: Some("tcp".to_string()),
            local_addr: "127.0.0.1:1".to_string(),
            remote_port: 60080,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn stale_session_cannot_unregister_new_session_services() {
        let state = SharedServiceState::new(60080, 60080, 0);
        let (old_tx, _old_rx) = mpsc::unbounded_channel();
        state
            .register_services("client", old_tx.clone(), vec![test_service()])
            .await;

        let (new_tx, _new_rx) = mpsc::unbounded_channel();
        state
            .register_services("client", new_tx.clone(), vec![test_service()])
            .await;

        assert!(!state.unregister_client_if_current("client", &old_tx).await);
        assert!(state.get_service_by_name("session-test").await.is_some());
        assert!(state.is_current_control("client", &new_tx).await);

        assert!(state.unregister_client_if_current("client", &new_tx).await);
        assert!(state.get_service_by_name("session-test").await.is_none());
    }

    #[tokio::test]
    async fn tunnel_timeout_removes_pending_work() {
        let state = SharedServiceState::new(60080, 60080, 0);
        let (control_tx, mut control_rx) = mpsc::unbounded_channel();
        state
            .register_services("client", control_tx, vec![test_service()])
            .await;
        let service = state.get_service_by_name("session-test").await.unwrap();

        let waiting_state = state.clone();
        let waiter = tokio::spawn(async move {
            waiting_state
                .open_tunnel(&service, None, None, Duration::from_millis(25))
                .await
        });

        let open = timeout(Duration::from_secs(1), control_rx.recv())
            .await
            .unwrap()
            .expect("open request should be sent");
        assert_eq!(open.ty, "open");

        let error = match waiter.await.unwrap() {
            Ok(_) => panic!("pending tunnel should time out"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("tunnel open timeout"));
        assert!(state.pending_work.read().await.is_empty());
    }
}
