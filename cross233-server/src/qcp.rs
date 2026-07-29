use crate::auth::AuthState;
use crate::control::handle_tunnel_connection;
use crate::service::{SharedServiceState, TunnelStream, TUNNEL_OPEN_TIMEOUT};
use cross233_qcp::{into_stream, listen, Conn, Stream};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufStream};
use tokio::time::{timeout, Duration};
use tokio_rustls::TlsAcceptor;

const MAX_FIRST_MESSAGE_BYTES: usize = 16 * 1024;

pub async fn run_qcp_listener(state: Arc<SharedServiceState>, addr: &str) -> anyhow::Result<()> {
    let mut listener = listen(addr).await?;
    tracing::info!(addr = %addr, "QCP listening");
    loop {
        let conn = match listener.accept().await {
            Some(c) => c,
            None => break,
        };
        let state = state.clone();
        tokio::spawn(async move {
            let _ = handle_qcp_conn(state, conn).await;
        });
    }
    Ok(())
}

/// Accept TLS-protected client tunnel connections over QCP.
///
/// This intentionally uses a dedicated UDP port. The older qcp_port listener
/// is a public service protocol and must not be mixed with authenticated
/// client-to-server tunnel traffic.
pub async fn run_qcp_tunnel_listener(
    state: Arc<SharedServiceState>,
    addr: &str,
    acceptor: TlsAcceptor,
    auth: AuthState,
    handshake_timeout_secs: u32,
) -> anyhow::Result<()> {
    let mut listener = listen(addr).await?;
    let handshake_timeout = Duration::from_secs(handshake_timeout_secs.max(1) as u64);
    tracing::info!(addr = %addr, "QCP tunnel listener started");

    loop {
        let conn = match listener.accept().await {
            Some(conn) => conn,
            None => break,
        };
        let state = state.clone();
        let acceptor = acceptor.clone();
        let auth = auth.clone();
        tokio::spawn(async move {
            if let Err(error) =
                handle_qcp_tunnel_connection(conn, state, acceptor, auth, handshake_timeout).await
            {
                tracing::debug!("QCP tunnel connection ended: {error}");
            }
        });
    }
    Ok(())
}

async fn handle_qcp_tunnel_connection(
    conn: Conn,
    state: Arc<SharedServiceState>,
    acceptor: TlsAcceptor,
    auth: AuthState,
    handshake_timeout: Duration,
) -> anyhow::Result<()> {
    let qcp = into_stream(conn, Stream::Batch);
    let tls = timeout(handshake_timeout, acceptor.accept(qcp))
        .await
        .map_err(|_| anyhow::anyhow!("QCP TLS handshake timeout"))?
        .map_err(|error| anyhow::anyhow!("QCP TLS handshake: {error}"))?;
    let mut reader = BufStream::new(tls);
    let first = timeout(handshake_timeout, read_first_message(&mut reader))
        .await
        .map_err(|_| anyhow::anyhow!("QCP tunnel hello timeout"))??;
    if first.ty != "tunnel" {
        return Err(anyhow::anyhow!(
            "QCP tunnel listener only accepts tunnel messages"
        ));
    }
    handle_tunnel_connection(reader, first, auth, state).await
}

async fn read_first_message<S>(
    stream: &mut BufStream<S>,
) -> anyhow::Result<cross233_protocol::Message>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut line = Vec::with_capacity(256);
    loop {
        let byte = stream.read_u8().await?;
        if byte == b'\n' {
            break;
        }
        line.push(byte);
        if line.len() > MAX_FIRST_MESSAGE_BYTES {
            return Err(anyhow::anyhow!("QCP first message is too large"));
        }
    }
    serde_json::from_slice(&line).map_err(Into::into)
}

async fn handle_qcp_conn(state: Arc<SharedServiceState>, conn: Conn) -> anyhow::Result<()> {
    let mut stream = into_stream(conn, Stream::Batch);
    let mut len_buf = [0u8; 2];
    timeout(Duration::from_secs(10), stream.read_exact(&mut len_buf)).await??;
    let name_len = u16::from_be_bytes(len_buf) as usize;
    if name_len == 0 || name_len > 256 {
        return Err(anyhow::anyhow!("bad service name length"));
    }
    let mut name_buf = vec![0u8; name_len];
    timeout(Duration::from_secs(10), stream.read_exact(&mut name_buf)).await??;
    let service_name = String::from_utf8(name_buf)?;

    let entry = state
        .get_service_by_name(&service_name)
        .await
        .ok_or_else(|| anyhow::anyhow!("service not found"))?;

    if !entry.config.is_qcp() {
        return Err(anyhow::anyhow!("service is not configured for public QCP"));
    }

    if !entry.enabled.load(Ordering::Relaxed) || !entry.healthy.load(Ordering::Relaxed) {
        return Err(anyhow::anyhow!("service not available"));
    }

    if let Some(secret) = &entry.config.secret {
        let mut slen_buf = [0u8; 2];
        timeout(Duration::from_secs(10), stream.read_exact(&mut slen_buf)).await??;
        let slen = u16::from_be_bytes(slen_buf) as usize;
        if slen > 256 {
            return Err(anyhow::anyhow!("bad secret length"));
        }
        let mut sbuf = vec![0u8; slen];
        timeout(Duration::from_secs(10), stream.read_exact(&mut sbuf)).await??;
        let provided = String::from_utf8(sbuf)?;
        if provided != *secret {
            stream.write_all(b"\x00\x01\x01").await?;
            return Err(anyhow::anyhow!("bad secret"));
        }
    }

    stream.write_all(b"\x00\x01\x00").await?;
    stream.flush().await?;

    let mut tunnel: TunnelStream = state
        .open_tunnel(&entry, None, None, TUNNEL_OPEN_TIMEOUT)
        .await
        .map_err(|_| anyhow::anyhow!("tunnel open failed"))?;

    entry.current_conns.fetch_add(1, Ordering::Relaxed);
    match tokio::io::copy_bidirectional(&mut stream, &mut tunnel).await {
        Ok((tx, rx)) => {
            state.record_traffic(&service_name, tx, rx).await;
        }
        Err(_) => {}
    }
    entry.current_conns.fetch_sub(1, Ordering::Relaxed);
    Ok(())
}
