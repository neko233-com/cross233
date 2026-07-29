use anyhow::Result;
use cross233_protocol::{Message, Service};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};

pub struct UdpForwarder {
    sock: Arc<UdpSocket>,
    peers: Arc<Mutex<HashMap<SocketAddr, ()>>>,
}

impl UdpForwarder {
    pub async fn new(local_addr: &str) -> Result<Self> {
        let sock = UdpSocket::bind(local_addr).await?;
        Ok(Self {
            sock: Arc::new(sock),
            peers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    pub async fn run(
        &self,
        service: Service,
        remote_port: u16,
        out_tx: mpsc::UnboundedSender<Message>,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) {
        let mut buf = vec![0u8; 65536];
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                res = self.sock.recv_from(&mut buf) => {
                    match res {
                        Ok((n, addr)) => {
                            self.peers.lock().await.insert(addr, ());
                            let data = buf[..n].to_vec();
                            let msg = Message::new_udp(remote_port, &addr.to_string(), data, service.clone());
                            let _ = out_tx.send(msg);
                        }
                        Err(e) => {
                            tracing::debug!("udp recv error: {e}");
                            break;
                        }
                    }
                }
            }
        }
    }

    pub async fn handle_response(&self, addr_str: &str, data: &[u8]) {
        if let Ok(addr) = addr_str.parse::<SocketAddr>() {
            let _ = self.sock.send_to(data, addr).await;
        }
    }
}

pub async fn handle_udp_response(
    forwarders: &Arc<Mutex<HashMap<u16, Arc<UdpForwarder>>>>,
    remote_port: u16,
    addr: &str,
    data: &[u8],
) {
    let fwds = forwarders.lock().await;
    if let Some(fwd) = fwds.get(&remote_port) {
        fwd.handle_response(addr, data).await;
    }
}
