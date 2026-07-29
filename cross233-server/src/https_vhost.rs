use crate::service::{SharedServiceState, TUNNEL_OPEN_TIMEOUT};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration};

fn parse_sni(data: &[u8]) -> Option<String> {
    if data.len() < 44 {
        return None;
    }
    if data[0] != 0x16 {
        return None;
    }
    let handshake = &data[5..];
    if handshake[0] != 0x01 {
        return None;
    }
    let mut pos = 38;
    if pos >= handshake.len() {
        return None;
    }
    let session_id_len = handshake[pos] as usize;
    pos += 1 + session_id_len;
    if pos + 2 > handshake.len() {
        return None;
    }
    let cipher_len = u16::from_be_bytes([handshake[pos], handshake[pos + 1]]) as usize;
    pos += 2 + cipher_len;
    if pos + 1 > handshake.len() {
        return None;
    }
    let comp_len = handshake[pos] as usize;
    pos += 1 + comp_len;
    if pos + 2 > handshake.len() {
        return None;
    }
    let ext_len = u16::from_be_bytes([handshake[pos], handshake[pos + 1]]) as usize;
    pos += 2;
    while pos + 4 <= handshake.len() {
        let ext_type = u16::from_be_bytes([handshake[pos], handshake[pos + 1]]);
        let ext_size = u16::from_be_bytes([handshake[pos + 2], handshake[pos + 3]]) as usize;
        pos += 4;
        if pos + ext_size > handshake.len() {
            break;
        }
        if ext_type == 0 {
            let mut ep = pos;
            if ep + 2 > pos + ext_size {
                break;
            }
            let list_len = u16::from_be_bytes([handshake[ep], handshake[ep + 1]]) as usize;
            ep += 2;
            while ep + 3 <= pos + ext_size {
                let name_type = handshake[ep];
                let name_len = u16::from_be_bytes([handshake[ep + 1], handshake[ep + 2]]) as usize;
                ep += 3;
                if ep + name_len <= pos + ext_size && name_type == 0 {
                    if let Ok(name) = std::str::from_utf8(&handshake[ep..ep + name_len]) {
                        return Some(name.to_string());
                    }
                }
                ep += name_len;
            }
        }
        pos += ext_size;
    }
    None
}

pub async fn run_https_vhost(state: Arc<SharedServiceState>, addr: &str) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(addr = %addr, "HTTPS vhost listening");
    loop {
        let (tcp, src_addr) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                tracing::error!("https accept: {}", e);
                break;
            }
        };
        let state = state.clone();
        tokio::spawn(async move {
            let _ = handle_https_vhost_conn(state, tcp, src_addr).await;
        });
    }
    Ok(())
}

async fn handle_https_vhost_conn(
    state: Arc<SharedServiceState>,
    mut inbound: tokio::net::TcpStream,
    src_addr: SocketAddr,
) -> anyhow::Result<()> {
    let mut buf = vec![0u8; 4096];
    let mut total = 0usize;
    let mut sni: Option<String> = None;
    while total < 4096 {
        let n = timeout(Duration::from_secs(5), inbound.read(&mut buf[total..])).await??;
        if n == 0 {
            break;
        }
        total += n;
        if total >= 44 {
            if let Some(s) = parse_sni(&buf[..total]) {
                sni = Some(s);
                break;
            }
        }
    }

    let host = sni.unwrap_or_default();
    let entry = match state.get_service_by_host(&host).await {
        Some(e) if e.enabled.load(Ordering::Relaxed) && e.healthy.load(Ordering::Relaxed) => e,
        _ => return Ok(()),
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
        _ => return Ok(()),
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

    tunnel.write_all(&buf[..total]).await?;

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
