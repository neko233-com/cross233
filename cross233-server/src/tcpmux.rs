use crate::service::{SharedServiceState, TUNNEL_OPEN_TIMEOUT};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub async fn run_tcpmux(state: Arc<SharedServiceState>, addr: &str) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(addr = %addr, "TCPMUX listening");
    loop {
        let (tcp, src_addr) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let _ = handle_tcpmux_conn(state, tcp, src_addr).await;
        });
    }
}

async fn handle_tcpmux_conn(
    state: Arc<SharedServiceState>,
    mut inbound: tokio::net::TcpStream,
    src_addr: SocketAddr,
) -> anyhow::Result<()> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1];
    loop {
        let n = inbound.read(&mut tmp).await?;
        if n == 0 {
            return Err(anyhow::anyhow!("closed"));
        }
        buf.push(tmp[0]);
        if buf.len() >= 2 && &buf[buf.len() - 2..] == b"\r\n" {
            break;
        }
        if buf.len() > 8192 {
            return Err(anyhow::anyhow!("request too large"));
        }
    }

    let line = String::from_utf8_lossy(&buf).to_string();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    if !method.eq_ignore_ascii_case("CONNECT") {
        let _ = inbound.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 15\r\nConnection: close\r\n\r\nexpecting CONNECT").await;
        return Ok(());
    }

    let host = target.split(':').next().unwrap_or(target).to_string();
    let entry = match state.get_service_by_host(&host).await {
        Some(e) if e.enabled.load(Ordering::Relaxed) && e.healthy.load(Ordering::Relaxed) => e,
        _ => {
            let _ = inbound.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 13\r\nConnection: close\r\n\r\nno such host").await;
            return Ok(());
        }
    };

    let mut tunnel = match state
        .open_tunnel(
            &entry,
            Some(src_addr),
            inbound.local_addr().ok(),
            TUNNEL_OPEN_TIMEOUT,
        )
        .await
    {
        Ok(t) => t,
        _ => {
            let _ = inbound.write_all(b"HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 15\r\nConnection: close\r\n\r\ntunnel timeout").await;
            return Ok(());
        }
    };

    if entry.config.proxy_protocol {
        let version = entry
            .config
            .proxy_protocol_version
            .as_deref()
            .unwrap_or("v1");
        if let Some(dst) = inbound.local_addr().ok() {
            let header = crate::proxy_protocol::build_header(version, src_addr, dst);
            let _ = tunnel.write_all(&header).await;
        }
    }

    let _ = inbound
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await;

    let (mut trd, mut twr) = tokio::io::split(tunnel);
    let (mut ird, mut iwr) = tokio::io::split(inbound);
    let c2s = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut ird, &mut twr).await;
    });
    let s2c = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut trd, &mut iwr).await;
    });
    let _ = tokio::try_join!(c2s, s2c);
    Ok(())
}
