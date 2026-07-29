use crate::service::{SharedServiceState, TUNNEL_OPEN_TIMEOUT};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn extract_host(data: &[u8]) -> Option<String> {
    let mut headers_end = 0;
    for i in 0..data.len().saturating_sub(3) {
        if &data[i..i + 4] == b"\r\n\r\n" {
            headers_end = i;
            break;
        }
    }
    let header_str = std::str::from_utf8(&data[..headers_end]).ok()?;
    for line in header_str.split("\r\n") {
        if line.len() > 5 && line[..5].eq_ignore_ascii_case("host:") {
            let val = line[5..].trim();
            return Some(val.split(':').next().unwrap_or(val).to_string());
        }
    }
    None
}

fn check_basic_auth(data: &[u8], user: &str, pass: &str) -> bool {
    let mut headers_end = 0;
    for i in 0..data.len().saturating_sub(3) {
        if &data[i..i + 4] == b"\r\n\r\n" {
            headers_end = i;
            break;
        }
    }
    let header_str = match std::str::from_utf8(&data[..headers_end]) {
        Ok(s) => s,
        Err(_) => return false,
    };
    for line in header_str.split("\r\n") {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("authorization:") || lower.starts_with("proxy-authorization:") {
            let val = line.split_once(':').map_or("", |(_, value)| value).trim();
            if let Some(b64) = val
                .strip_prefix("Basic ")
                .or_else(|| val.strip_prefix("basic "))
            {
                if let Ok(decoded) = STANDARD.decode(b64.trim()) {
                    if let Ok(s) = String::from_utf8(decoded) {
                        let mut parts = s.splitn(2, ':');
                        let u = parts.next().unwrap_or("");
                        let p = parts.next().unwrap_or("");
                        return u == user && p == pass;
                    }
                }
            }
        }
    }
    false
}

pub async fn run_http_vhost(state: Arc<SharedServiceState>, addr: &str) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(addr = %addr, "HTTP vhost listening");
    loop {
        let (tcp, src_addr) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let _ = handle_http_vhost_conn(state, tcp, src_addr).await;
        });
    }
}

async fn handle_http_vhost_conn(
    state: Arc<SharedServiceState>,
    mut inbound: tokio::net::TcpStream,
    src_addr: SocketAddr,
) -> anyhow::Result<()> {
    let mut buf = Vec::with_capacity(8192);
    let mut tmp = [0u8; 1];
    loop {
        let n = inbound.read(&mut tmp).await?;
        if n == 0 {
            return Err(anyhow::anyhow!("connection closed before headers"));
        }
        buf.push(tmp[0]);
        if buf.len() >= 4 && &buf[buf.len() - 4..] == b"\r\n\r\n" {
            break;
        }
        if buf.len() > 65536 {
            return Err(anyhow::anyhow!("headers too large"));
        }
    }

    let host = extract_host(&buf).unwrap_or_default();

    let entry = match state.get_service_by_host(&host).await {
        Some(e) if e.enabled.load(Ordering::Relaxed) && e.healthy.load(Ordering::Relaxed) => e,
        _ => {
            let _ = inbound.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 13\r\nConnection: close\r\n\r\nno such vhost").await;
            return Ok(());
        }
    };

    if let (Some(user), Some(pass)) = (&entry.config.http_user, &entry.config.http_password) {
        if !check_basic_auth(&buf, user, pass) {
            let _ = inbound.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"cross233\"\r\nContent-Length: 14\r\nConnection: close\r\n\r\nauth required").await;
            return Ok(());
        }
    }

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
        if let Ok(dst) = inbound.local_addr() {
            let header = crate::proxy_protocol::build_header(version, src_addr, dst);
            let _ = tunnel.write_all(&header).await;
        }
    }

    if let Some(rewrite) = &entry.config.host_header_rewrite {
        let mut new_buf = Vec::with_capacity(buf.len() + 100);
        if let Ok(s) = std::str::from_utf8(&buf) {
            let mut first = true;
            for line in s.split("\r\n") {
                if first {
                    new_buf.extend_from_slice(line.as_bytes());
                    new_buf.extend_from_slice(b"\r\n");
                    first = false;
                } else if line.to_ascii_lowercase().starts_with("host:") {
                    new_buf.extend_from_slice(format!("Host: {}\r\n", rewrite).as_bytes());
                } else {
                    new_buf.extend_from_slice(line.as_bytes());
                    new_buf.extend_from_slice(b"\r\n");
                }
            }
            for (k, v) in &entry.config.request_headers {
                new_buf.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes());
            }
            buf = new_buf;
        }
    }

    tunnel.write_all(&buf).await?;

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
