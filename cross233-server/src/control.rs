use crate::auth::AuthState;
use crate::service::{SharedServiceState, STATIC_RESPONSE_CHUNK_MAX, TUNNEL_OPEN_TIMEOUT};
use crate::udp::UdpManager;
use cross233_protocol::Message;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufStream};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration, Instant};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

const MAX_INITIAL_MESSAGE_BYTES: usize = 16 * 1024;
const STATIC_CONTROL_HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const STATIC_CONTROL_FIRST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const STATIC_CONTROL_RESPONSE_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[allow(clippy::too_many_arguments)]
pub async fn run_control_listener(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    auth: AuthState,
    state: Arc<SharedServiceState>,
    udp_mgr: Option<Arc<UdpManager>>,
    qcp_port: u16,
    qcp_tunnel_port: u16,
    handshake_timeout: Duration,
) {
    loop {
        let (tcp, remote_addr) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                tracing::error!("control accept error: {}", e);
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };

        let acceptor = acceptor.clone();
        let auth = auth.clone();
        let state = state.clone();
        let udp_mgr = udp_mgr.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection(
                tcp,
                remote_addr,
                acceptor,
                auth,
                state,
                udp_mgr,
                qcp_port,
                qcp_tunnel_port,
                handshake_timeout,
            )
            .await
            {
                tracing::debug!("connection from {} ended: {}", remote_addr, e);
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_connection(
    tcp: TcpStream,
    remote_addr: SocketAddr,
    acceptor: TlsAcceptor,
    auth: AuthState,
    state: Arc<SharedServiceState>,
    udp_mgr: Option<Arc<UdpManager>>,
    qcp_port: u16,
    qcp_tunnel_port: u16,
    handshake_timeout: Duration,
) -> anyhow::Result<()> {
    let tls = match timeout(handshake_timeout, acceptor.accept(tcp)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(error)) => {
            tracing::warn!(remote = %remote_addr, error = %error, "control TLS handshake failed");
            return Err(anyhow::anyhow!("tls accept: {error}"));
        }
        Err(_) => {
            tracing::warn!(remote = %remote_addr, timeout_secs = handshake_timeout.as_secs(), "control TLS handshake timed out");
            return Err(anyhow::anyhow!("tls accept timeout"));
        }
    };
    tracing::debug!(remote = %remote_addr, "control TLS handshake complete");

    let mut reader = BufStream::new(tls);
    let line = timeout(handshake_timeout, read_initial_message(&mut reader))
        .await
        .map_err(|_| anyhow::anyhow!("initial message timeout"))??;
    let first_msg: Message =
        serde_json::from_str(line.trim()).map_err(|e| anyhow::anyhow!("bad first msg: {}", e))?;

    match first_msg.ty.as_str() {
        "client" => {
            handle_control_session(
                reader,
                first_msg,
                remote_addr,
                auth,
                state,
                udp_mgr,
                qcp_port,
                qcp_tunnel_port,
            )
            .await?;
        }
        "tunnel" => {
            handle_tunnel_connection(reader, first_msg, auth, state).await?;
        }
        "visitor" => {
            handle_visitor_connection(reader, first_msg, auth, state).await?;
        }
        other => {
            tracing::debug!(
                "unknown first message type '{}' from {}",
                other,
                remote_addr
            );
        }
    }
    Ok(())
}

async fn read_initial_message<S>(stream: &mut S) -> anyhow::Result<String>
where
    S: AsyncRead + Unpin,
{
    let mut line = Vec::with_capacity(256);
    loop {
        let byte = stream.read_u8().await?;
        if byte == b'\n' {
            break;
        }
        if line.len() >= MAX_INITIAL_MESSAGE_BYTES {
            return Err(anyhow::anyhow!(
                "initial message exceeds {MAX_INITIAL_MESSAGE_BYTES} bytes"
            ));
        }
        line.push(byte);
    }
    String::from_utf8(line).map_err(Into::into)
}

#[allow(clippy::too_many_arguments)]
async fn handle_control_session(
    mut stream: BufStream<TlsStream<TcpStream>>,
    first_msg: Message,
    remote_addr: SocketAddr,
    auth: AuthState,
    state: Arc<SharedServiceState>,
    udp_mgr: Option<Arc<UdpManager>>,
    qcp_port: u16,
    qcp_tunnel_port: u16,
) -> anyhow::Result<()> {
    let cid = first_msg.client_id.clone().unwrap_or_default();
    if cid.is_empty() {
        return Err(anyhow::anyhow!("empty client_id"));
    }

    let nonce = auth.create_challenge(&cid).await;
    send_line(
        &mut stream,
        &serde_json::to_string(&Message::new_challenge(&nonce))?,
    )
    .await?;

    let mut line = String::new();
    line.clear();
    timeout(Duration::from_secs(30), stream.read_line(&mut line))
        .await
        .map_err(|_| anyhow::anyhow!("auth timeout"))??;

    let auth_msg: Message =
        serde_json::from_str(line.trim()).map_err(|e| anyhow::anyhow!("bad auth msg: {}", e))?;

    if auth_msg.ty != "auth" {
        send_line(
            &mut stream,
            &serde_json::to_string(&Message::new_error("expected auth"))?,
        )
        .await?;
        return Err(anyhow::anyhow!("expected auth, got {}", auth_msg.ty));
    }

    let proof = auth_msg
        .proof
        .ok_or_else(|| anyhow::anyhow!("missing proof"))?;
    let ok = auth.verify(&cid, "client", &cid, &cid, &proof).await;
    if !ok {
        send_line(
            &mut stream,
            &serde_json::to_string(&Message::new_error("auth failed"))?,
        )
        .await?;
        tracing::warn!(client_id = %cid, "auth failed");
        return Err(anyhow::anyhow!("auth failed"));
    }

    let (control_tx, mut control_rx) = mpsc::unbounded_channel::<Message>();

    let services = first_msg.services.clone();
    let assigned = state
        .register_services(&cid, control_tx.clone(), services)
        .await;

    for svc in &assigned {
        match svc.effective_type() {
            "tcp" | "static" | "" => {
                if !svc.is_vhost()
                    && svc.remote_port != 0
                    && !state.is_listener_active(svc.remote_port).await
                {
                    state.mark_listener_active(svc.remote_port).await;
                    spawn_tcp_listener_for_service(
                        state.clone(),
                        svc.name.clone(),
                        svc.remote_port,
                    );
                }
            }
            "udp" => {
                if let Some(mgr) = &udp_mgr {
                    if svc.remote_port != 0 {
                        let _ = mgr.start_service(&svc.name, svc.remote_port).await;
                    }
                }
            }
            _ => {}
        }
    }

    let ready_msg = Message::new_ready(assigned, qcp_port, qcp_tunnel_port);
    send_line(&mut stream, &serde_json::to_string(&ready_msg)?).await?;

    tracing::info!(client_id = %cid, addr = %remote_addr, "client authenticated");

    let ping_tx = control_tx.clone();
    let ping_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(20));
        interval.tick().await;
        loop {
            interval.tick().await;
            if ping_tx.send(Message::new_ping()).is_err() {
                break;
            }
        }
    });

    let mut last_activity = Instant::now();
    let mut msg_line = String::new();
    loop {
        msg_line.clear();
        tokio::select! {
            read_res = timeout(Duration::from_secs(90), stream.read_line(&mut msg_line)) => {
                match read_res {
                    Ok(Ok(0)) => break,
                    Ok(Ok(_)) => {
                        let trimmed = msg_line.trim();
                        if trimmed.is_empty() {
                            let mut out_buf = Vec::new();
                            while let Ok(msg) = control_rx.try_recv() {
                                if let Ok(s) = serde_json::to_string(&msg) {
                                    out_buf.extend_from_slice(s.as_bytes());
                                    out_buf.push(b'\n');
                                }
                            }
                            if !out_buf.is_empty() {
                                stream.write_all(&out_buf).await?;
                                stream.flush().await?;
                            }
                            continue;
                        }
                        last_activity = Instant::now();
                        if let Ok(msg) = serde_json::from_str::<Message>(trimmed) {
                            match msg.ty.as_str() {
                                "pong" => {}
                                "ping" => {
                                    let _ = control_tx.send(Message::new_pong());
                                }
                                "health" => {
                                    if let (Some(name), Some(healthy)) = (msg.service_name, msg.healthy) {
                                        state.report_health(&name, healthy).await;
                                    }
                                }
                                "static_response" => {
                                    if let Some(id) = msg.id.as_deref() {
                                        let delivered = state
                                            .deliver_static_response(
                                                &control_tx,
                                                id,
                                                msg.data.unwrap_or_default(),
                                                msg.eof.unwrap_or(false),
                                            )
                                            .await;
                                        if !delivered {
                                            tracing::trace!(request_id = %id, client_id = %cid, "discarded static control response");
                                        }
                                    }
                                }
                                "udp_response" => {
                                    if let (Some(rport), Some(addr), Some(data)) =
                                        (msg.remote_port, msg.address.as_ref(), msg.data.as_ref())
                                    {
                                        if let Some(mgr) = &udp_mgr {
                                            if let Some(svc_name) = state.get_service_name_by_port(rport).await {
                                                mgr.forward_response(&svc_name, addr, data).await;
                                            }
                                        }
                                    }
                                }
                                _ => {
                                    tracing::trace!(ty = %msg.ty, client_id = %cid, "msg on control");
                                }
                            }
                        }
                        let mut out_buf = Vec::new();
                        while let Ok(msg) = control_rx.try_recv() {
                            if let Ok(s) = serde_json::to_string(&msg) {
                                out_buf.extend_from_slice(s.as_bytes());
                                out_buf.push(b'\n');
                            }
                        }
                        if !out_buf.is_empty() {
                            stream.write_all(&out_buf).await?;
                            stream.flush().await?;
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::debug!(client_id = %cid, "read error: {}", e);
                        break;
                    }
                    Err(_) => {
                        if last_activity.elapsed() > Duration::from_secs(60) {
                            tracing::warn!(client_id = %cid, "keepalive timeout");
                            break;
                        }
                        let mut out_buf = Vec::new();
                        while let Ok(msg) = control_rx.try_recv() {
                            if let Ok(s) = serde_json::to_string(&msg) {
                                out_buf.extend_from_slice(s.as_bytes());
                                out_buf.push(b'\n');
                            }
                        }
                        if !out_buf.is_empty() {
                            stream.write_all(&out_buf).await?;
                            stream.flush().await?;
                        }
                    }
                }
            }
            Some(msg) = control_rx.recv() => {
                if let Ok(s) = serde_json::to_string(&msg) {
                    send_line(&mut stream, &s).await?;
                }
            }
        }
    }

    ping_task.abort();
    state.cancel_static_responses_for_control(&control_tx).await;

    if state.is_current_control(&cid, &control_tx).await {
        if let Some(mgr) = &udp_mgr {
            let client_svcs = state
                .client_services
                .read()
                .await
                .get(&cid)
                .cloned()
                .unwrap_or_default();
            for name in &client_svcs {
                mgr.stop_service(name).await;
            }
        }

        if state.unregister_client_if_current(&cid, &control_tx).await {
            tracing::info!(client_id = %cid, "client disconnected");
        }
    } else {
        tracing::debug!(client_id = %cid, "stale client session disconnected");
    }
    Ok(())
}

pub(crate) async fn handle_tunnel_connection<S>(
    mut reader: BufStream<S>,
    first_msg: Message,
    auth: AuthState,
    state: Arc<SharedServiceState>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let tunnel_id = first_msg.id.clone().unwrap_or_default();
    if tunnel_id.is_empty() {
        return Err(anyhow::anyhow!("empty tunnel id"));
    }

    let nonce = auth
        .create_challenge(&format!("tunnel:{}", tunnel_id))
        .await;

    let proof = if let Some(p) = first_msg.proof {
        p
    } else {
        send_line(
            &mut reader,
            &serde_json::to_string(&Message::new_challenge(&nonce))?,
        )
        .await?;
        let mut line = String::new();
        timeout(Duration::from_secs(15), reader.read_line(&mut line))
            .await
            .map_err(|_| anyhow::anyhow!("tunnel auth timeout"))??;
        let amsg: Message = serde_json::from_str(line.trim())?;
        amsg.proof.ok_or_else(|| anyhow::anyhow!("missing proof"))?
    };

    let ok = auth
        .verify(
            &format!("tunnel:{}", tunnel_id),
            "tunnel",
            "",
            &tunnel_id,
            &proof,
        )
        .await;
    if !ok {
        send_line(
            &mut reader,
            &serde_json::to_string(&Message::new_reject(&tunnel_id, "auth failed"))?,
        )
        .await?;
        state.reject_tunnel(&tunnel_id, "auth failed").await;
        return Err(anyhow::anyhow!("tunnel auth failed"));
    }

    let pending = { state.pending_work.write().await.remove(&tunnel_id) };
    if let Some(tx) = pending {
        send_line(
            &mut reader,
            &serde_json::to_string(&Message::new_ready(Vec::new(), 0, 0))?,
        )
        .await?;
        let _ = tx.send(Ok(Box::new(reader)));
        Ok(())
    } else {
        send_line(
            &mut reader,
            &serde_json::to_string(&Message::new_reject(&tunnel_id, "no pending work"))?,
        )
        .await?;
        Err(anyhow::anyhow!("no pending work for tunnel {}", tunnel_id))
    }
}

async fn handle_visitor_connection(
    mut reader: BufStream<TlsStream<TcpStream>>,
    first_msg: Message,
    auth: AuthState,
    _state: Arc<SharedServiceState>,
) -> anyhow::Result<()> {
    let cid = first_msg.client_id.clone().unwrap_or_default();
    let svc_name = first_msg.service_name.clone().unwrap_or_default();
    let id = first_msg.id.clone().unwrap_or_default();

    let nonce = auth.create_challenge(&format!("visitor:{}", id)).await;
    let proof = if let Some(p) = first_msg.proof {
        Some(p)
    } else {
        send_line(
            &mut reader,
            &serde_json::to_string(&Message::new_challenge(&nonce))?,
        )
        .await?;
        let mut line = String::new();
        match timeout(Duration::from_secs(15), reader.read_line(&mut line)).await {
            Ok(Ok(_)) => serde_json::from_str::<Message>(line.trim())
                .ok()
                .and_then(|m| m.proof),
            _ => None,
        }
    };

    if let Some(proof) = proof {
        let ok = auth
            .verify(&format!("visitor:{}", id), "visitor", &cid, &id, &proof)
            .await;
        if ok {
            tracing::info!(service = %svc_name, visitor = %cid, "visitor authenticated");
            return Ok(());
        }
    }
    Err(anyhow::anyhow!("visitor auth failed"))
}

async fn send_line<W: AsyncWriteExt + Unpin>(w: &mut W, s: &str) -> std::io::Result<()> {
    w.write_all(s.as_bytes()).await?;
    w.write_all(b"\n").await?;
    w.flush().await
}

pub fn spawn_tcp_listener_for_service(
    state: Arc<SharedServiceState>,
    service_name: String,
    port: u16,
) {
    let addr = format!("0.0.0.0:{}", port);
    tokio::spawn(async move {
        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(service = %service_name, addr = %addr, "bind failed: {}", e);
                state.remove_listener_handle(port).await;
                return;
            }
        };
        tracing::info!(service = %service_name, addr = %addr, "TCP listener started");
        loop {
            let (tcp, src_addr) = match listener.accept().await {
                Ok(x) => x,
                Err(e) => {
                    tracing::error!(service = %service_name, "accept error: {}", e);
                    if e.kind() == std::io::ErrorKind::InvalidInput {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    break;
                }
            };
            let state = state.clone();
            tokio::spawn(async move {
                let Some(svc_name) = state.get_service_name_by_port(port).await else {
                    tracing::warn!(port, peer = %src_addr, "inbound TCP connection has no registered service");
                    return;
                };
                if let Err(error) = handle_inbound_tcp(state, svc_name.clone(), tcp, src_addr).await
                {
                    tracing::warn!(service = %svc_name, peer = %src_addr, "inbound TCP tunnel failed: {error}");
                }
            });
        }
        state.remove_listener_handle(port).await;
    });
}

async fn handle_inbound_tcp(
    state: Arc<SharedServiceState>,
    service_name: String,
    mut inbound: TcpStream,
    src_addr: SocketAddr,
) -> anyhow::Result<()> {
    let entry = state
        .get_service_by_name(&service_name)
        .await
        .ok_or_else(|| anyhow::anyhow!("service not found"))?;

    if !entry.enabled.load(Ordering::Relaxed) || !entry.healthy.load(Ordering::Relaxed) {
        return Err(anyhow::anyhow!("service not available"));
    }

    if entry.config.max_connections > 0 {
        let cur = entry.current_conns.load(Ordering::Relaxed);
        if cur >= entry.config.max_connections as usize {
            return Err(anyhow::anyhow!("max connections reached"));
        }
    }

    entry.current_conns.fetch_add(1, Ordering::Relaxed);

    let result = if entry.config.effective_type() == "static" && !entry.config.proxy_protocol {
        handle_static_inbound(
            state.clone(),
            &service_name,
            entry.clone(),
            &mut inbound,
            src_addr,
        )
        .await
    } else {
        relay_inbound_tunnel(
            state.clone(),
            &service_name,
            entry.clone(),
            &mut inbound,
            src_addr,
            &[],
        )
        .await
    };

    entry.current_conns.fetch_sub(1, Ordering::Relaxed);
    result
}

async fn handle_static_inbound(
    state: Arc<SharedServiceState>,
    service_name: &str,
    entry: Arc<crate::service::ServiceEntry>,
    inbound: &mut TcpStream,
    src_addr: SocketAddr,
) -> anyhow::Result<()> {
    let capture = capture_static_request_headers(inbound).await?;

    if let Some(request) = capture.control_request.as_ref() {
        match relay_static_control_response(
            state.clone(),
            service_name,
            entry.clone(),
            inbound,
            request,
        )
        .await
        {
            Ok(true) => return Ok(()),
            Ok(false) => {
                tracing::debug!(service = %service_name, "static control response unavailable; using tunnel fallback");
            }
            Err(error) => return Err(error),
        }
    }

    // A client can reconnect after the header was read. Resolve once more so
    // fallback does not send an open request through a stale control channel.
    let current_entry = state
        .get_service_by_name(service_name)
        .await
        .ok_or_else(|| anyhow::anyhow!("service disconnected before tunnel fallback"))?;
    relay_inbound_tunnel(
        state,
        service_name,
        current_entry,
        inbound,
        src_addr,
        &capture.replay,
    )
    .await
}

async fn relay_static_control_response(
    state: Arc<SharedServiceState>,
    service_name: &str,
    entry: Arc<crate::service::ServiceEntry>,
    inbound: &mut TcpStream,
    request: &[u8],
) -> anyhow::Result<bool> {
    let (request_id, mut responses) = match state
        .request_static_response(&entry, request.to_vec())
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!(service = %service_name, "static control request unavailable: {error}");
            return Ok(false);
        }
    };

    let first = match timeout(STATIC_CONTROL_FIRST_RESPONSE_TIMEOUT, responses.recv()).await {
        Ok(Some(chunk)) if !chunk.data.is_empty() => chunk,
        Ok(Some(_)) | Ok(None) | Err(_) => {
            state.cancel_static_response(&request_id).await;
            return Ok(false);
        }
    };

    let mut sent = 0_u64;
    if let Err(error) = inbound.write_all(&first.data).await {
        state.cancel_static_response(&request_id).await;
        return Err(error.into());
    }
    sent += first.data.len() as u64;
    let mut eof = first.eof;

    while !eof {
        let chunk = match timeout(STATIC_CONTROL_RESPONSE_IDLE_TIMEOUT, responses.recv()).await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => {
                state.cancel_static_response(&request_id).await;
                return Err(anyhow::anyhow!("static control response ended before eof"));
            }
            Err(_) => {
                state.cancel_static_response(&request_id).await;
                return Err(anyhow::anyhow!("static control response idle timeout"));
            }
        };
        if let Err(error) = inbound.write_all(&chunk.data).await {
            state.cancel_static_response(&request_id).await;
            return Err(error.into());
        }
        sent += chunk.data.len() as u64;
        eof = chunk.eof;
    }
    inbound.flush().await?;
    state
        .record_traffic(service_name, sent, request.len() as u64)
        .await;
    Ok(true)
}

async fn relay_inbound_tunnel(
    state: Arc<SharedServiceState>,
    service_name: &str,
    entry: Arc<crate::service::ServiceEntry>,
    inbound: &mut TcpStream,
    src_addr: SocketAddr,
    replay: &[u8],
) -> anyhow::Result<()> {
    let mut tunnel = state
        .open_tunnel(
            &entry,
            Some(src_addr),
            inbound.local_addr().ok(),
            TUNNEL_OPEN_TIMEOUT,
        )
        .await?;

    if entry.config.proxy_protocol {
        let version = entry
            .config
            .proxy_protocol_version
            .as_deref()
            .unwrap_or("v1");
        if let Ok(dst) = inbound.local_addr() {
            let header = crate::proxy_protocol::build_header(version, src_addr, dst);
            tunnel.write_all(&header).await?;
        }
    }
    if !replay.is_empty() {
        tunnel.write_all(replay).await?;
        tunnel.flush().await?;
    }

    match tokio::io::copy_bidirectional(&mut tunnel, inbound).await {
        Ok((tx, rx)) => {
            state.record_traffic(service_name, tx, rx).await;
            Ok(())
        }
        Err(error) => Err(anyhow::anyhow!("copy error: {}", error)),
    }
}

struct StaticRequestCapture {
    control_request: Option<Vec<u8>>,
    replay: Vec<u8>,
}

async fn capture_static_request_headers(
    inbound: &mut TcpStream,
) -> anyhow::Result<StaticRequestCapture> {
    let deadline = Instant::now() + STATIC_CONTROL_HEADER_TIMEOUT;
    let mut replay = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(StaticRequestCapture {
                control_request: None,
                replay,
            });
        }
        let count = match timeout(remaining, inbound.read(&mut buffer)).await {
            Ok(Ok(0)) | Err(_) => {
                return Ok(StaticRequestCapture {
                    control_request: None,
                    replay,
                });
            }
            Ok(Ok(count)) => count,
            Ok(Err(error)) => return Err(error.into()),
        };
        replay.extend_from_slice(&buffer[..count]);

        if let Some(end) = replay
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
        {
            if end <= STATIC_RESPONSE_CHUNK_MAX {
                return Ok(StaticRequestCapture {
                    control_request: Some(replay[..end].to_vec()),
                    replay,
                });
            }
        }
        if replay.len() >= STATIC_RESPONSE_CHUNK_MAX {
            return Ok(StaticRequestCapture {
                control_request: None,
                replay,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cross233_protocol::Service;
    use tokio::sync::mpsc;

    fn unused_tcp_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    async fn next_open(rx: &mut mpsc::UnboundedReceiver<Message>) -> Message {
        timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .expect("listener should request a tunnel")
    }

    #[tokio::test]
    async fn tcp_listener_survives_client_reconnect() {
        let port = unused_tcp_port();
        let state = SharedServiceState::new(port, port, 0);
        let service = Service {
            name: "reconnect-test".to_string(),
            ty: Some("tcp".to_string()),
            local_addr: "127.0.0.1:1".to_string(),
            remote_port: port,
            ..Default::default()
        };

        let (first_tx, mut first_rx) = mpsc::unbounded_channel();
        state
            .register_services("client", first_tx, vec![service.clone()])
            .await;
        state.mark_listener_active(port).await;
        spawn_tcp_listener_for_service(state.clone(), service.name.clone(), port);

        let first_stream = timeout(Duration::from_secs(1), async {
            loop {
                match TcpStream::connect(("127.0.0.1", port)).await {
                    Ok(stream) => break stream,
                    Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
        })
        .await
        .expect("listener should bind");
        let first_open = next_open(&mut first_rx).await;
        state
            .reject_tunnel(first_open.id.as_deref().unwrap(), "test complete")
            .await;
        drop(first_stream);

        state.unregister_client("client").await;
        assert!(state.is_listener_active(port).await);

        let (second_tx, mut second_rx) = mpsc::unbounded_channel();
        state
            .register_services("client", second_tx, vec![service])
            .await;

        let second_stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let second_open = next_open(&mut second_rx).await;
        state
            .reject_tunnel(second_open.id.as_deref().unwrap(), "test complete")
            .await;
        drop(second_stream);
    }
}
