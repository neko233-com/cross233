use anyhow::{anyhow, Context, Result};
use cross233_protocol::{Message, Service};
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::{interval, sleep, timeout, Instant};
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio_rustls::TlsConnector;

use crate::config::ClientConfig;
use crate::transport::{TransportCache, TransportKind, TransportMode};
use crate::tunnel;
use crate::web::WebState;

// A healthy control/data path completes the TLS handshake quickly.  When an
// upstream device intermittently drops a new TLS connection, retry the same
// requested tunnel before auto mode falls back to QCP.  The attempts are
// sequential: the server keeps one pending tunnel id, and the HMAC challenge
// is never raced by concurrent connections.
const TCP_TUNNEL_ATTEMPTS: u8 = 3;
const TCP_TUNNEL_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
const TCP_TUNNEL_RETRY_DELAY: Duration = Duration::from_millis(100);

#[derive(Debug)]
struct NoVerifier;

impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
}

pub fn build_tls_config(cfg: &ClientConfig) -> Result<Arc<rustls::ClientConfig>> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let config_builder = if cfg.insecure {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
    } else {
        let mut roots = RootCertStore::empty();
        if cfg.ca_file.is_empty() {
            return Err(anyhow!("ca_file is required unless insecure=true"));
        }
        let ca_data =
            std::fs::read(&cfg.ca_file).with_context(|| format!("read CA file {}", cfg.ca_file))?;
        let mut cursor = Cursor::new(ca_data);
        for cert in rustls_pemfile::certs(&mut cursor) {
            let cert = cert.context("parse CA cert")?;
            roots.add(cert).context("add CA cert")?;
        }
        rustls::ClientConfig::builder().with_root_certificates(roots)
    };

    let config = if !cfg.key_file.is_empty() {
        let key_data = std::fs::read(&cfg.key_file)
            .with_context(|| format!("read key file {}", cfg.key_file))?;
        let mut cursor = Cursor::new(key_data);
        let certs: Vec<_> = rustls_pemfile::certs(&mut cursor)
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("parse client certs")?;
        cursor.set_position(0);
        let key = rustls_pemfile::private_key(&mut cursor)?
            .ok_or_else(|| anyhow!("no private key in {}", cfg.key_file))?;
        if certs.is_empty() {
            return Err(anyhow!("no certificate in {}", cfg.key_file));
        }
        config_builder.with_client_auth_cert(certs, key)?
    } else {
        config_builder.with_no_client_auth()
    };

    Ok(Arc::new(config))
}

pub struct Client {
    config: Arc<RwLock<ClientConfig>>,
    tls_config: Arc<rustls::ClientConfig>,
    web_state: WebState,
    shutdown: Arc<tokio::sync::Notify>,
}

impl Client {
    pub fn new(
        config: ClientConfig,
        web_state: WebState,
        shutdown: Arc<tokio::sync::Notify>,
    ) -> Result<Self> {
        let tls_config = build_tls_config(&config)?;
        Ok(Self {
            config: Arc::new(RwLock::new(config)),
            tls_config,
            web_state,
            shutdown,
        })
    }

    pub async fn run(&self) -> Result<()> {
        let mut backoff = 3u64;
        // Keep service registration responsive when an upstream edge
        // intermittently drops new TLS handshakes.  Longer exponential waits
        // turn a short network blip into a prolonged public-service outage.
        let max_backoff = 6u64;

        loop {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    tracing::info!("shutdown signal received");
                    return Ok(());
                }
                r = self.session() => {
                    {
                        let mut s = self.web_state.write().await;
                        s.connected = false;
                    }
                    match r {
                        Ok(()) => {
                            tracing::info!("session closed cleanly");
                            backoff = 3;
                        }
                        Err(e) => {
                            tracing::warn!("session error: {e:#}; reconnecting in {backoff}s");
                        }
                    }
                }
            }

            tokio::select! {
                _ = self.shutdown.notified() => return Ok(()),
                _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
            }
            backoff = (backoff * 2).min(max_backoff);
        }
    }

    async fn session(&self) -> Result<()> {
        let cfg = self.config.read().await.clone();
        let server_addr = cfg.server.clone();
        let server_name = cfg.server_name.clone();
        let client_id = cfg.client_id.clone();
        let auth_key = cfg.auth_key.clone();
        let qcp_port_cfg = cfg.qcp_port;
        let qcp_tunnel_port_cfg = cfg.qcp_tunnel_port;
        let transport_mode = TransportMode::parse(&cfg.transport)?;
        let transport_cache = Arc::new(TransportCache::new(
            cfg.transport_cache_file.clone(),
            cfg.transport_cache_ttl_secs,
        ));
        let qcp_probe_timeout = Duration::from_millis(cfg.transport_probe_timeout_ms.max(250));
        let services = cfg.enabled_services();

        tracing::info!(server = %server_addr, client_id = %client_id, "connecting to server");

        let tcp = TcpStream::connect(&server_addr)
            .await
            .with_context(|| format!("connect to {server_addr}"))?;
        tcp.set_nodelay(true)?;

        let connector = TlsConnector::from(self.tls_config.clone());
        let name = ServerName::try_from(server_name.clone())
            .map_err(|_| anyhow!("invalid server name: {server_name}"))?;
        let mut tls = connector
            .connect(name, tcp)
            .await
            .context("tls handshake")?;

        tunnel::write_ndjson(
            &mut tls,
            &Message::new_client_hello(&client_id, services.clone()),
        )
        .await?;
        tracing::debug!("sent client hello");

        let challenge = tunnel::read_ndjson(&mut tls).await?;
        if challenge.ty != "challenge" {
            return Err(anyhow!("expected challenge, got {}", challenge.ty));
        }
        let nonce = challenge.nonce.as_deref().unwrap_or("");

        let proof = crate::auth::compute_proof(&auth_key, "client", &client_id, &client_id, nonce);
        tunnel::write_ndjson(&mut tls, &Message::new_auth(&proof)).await?;

        let ready = tunnel::read_ndjson(&mut tls).await?;
        if ready.ty == "error" {
            return Err(anyhow!(
                "auth rejected: {}",
                ready.error.as_deref().unwrap_or("unknown")
            ));
        }
        if ready.ty != "ready" {
            return Err(anyhow!("expected ready, got {}", ready.ty));
        }

        let assigned_services = ready.services.clone();
        let srv_qcp_port = ready.qcp_port.unwrap_or(qcp_port_cfg);
        let srv_qcp_tunnel_port = ready.qcp_tunnel_port.unwrap_or(qcp_tunnel_port_cfg);
        let transport_cache_key =
            TransportCache::key(&server_addr, &server_name, srv_qcp_tunnel_port);
        tracing::info!(
            qcp_port = srv_qcp_port,
            qcp_tunnel_port = srv_qcp_tunnel_port,
            transport = %cfg.transport,
            "authenticated, services registered"
        );

        {
            let mut s = self.web_state.write().await;
            s.connected = true;
            s.services = assigned_services.clone();
            s.qcp_port = srv_qcp_port;
            s.qcp_tunnel_port = srv_qcp_tunnel_port;
            s.transport = cfg.transport.clone();
            s.active_tunnel_transport.clear();
            s.transport_cache_file = transport_cache.path().to_string_lossy().into_owned();
        }

        let (rd, mut wr) = tokio::io::split(tls);
        let mut rd = BufReader::new(rd);

        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
        let (shutdown_tx, _shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

        let writer_shutdown = shutdown_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                if write_ndjson(&mut wr, &msg).await.is_err() {
                    break;
                }
            }
            let _ = writer_shutdown.send(());
        });

        let mut keepalive = interval(Duration::from_secs(20));
        keepalive.tick().await;
        let mut last_pong = Instant::now();
        let pong_timeout = Duration::from_secs(60);

        let udp_forwarders: Arc<
            Mutex<std::collections::HashMap<u16, Arc<crate::udp::UdpForwarder>>>,
        > = Arc::new(Mutex::new(std::collections::HashMap::new()));

        for svc in &assigned_services {
            if svc.is_udp() && !svc.is_private() && svc.remote_port > 0 {
                if let Ok(fwd) = crate::udp::UdpForwarder::new(&svc.local_addr).await {
                    let fwd = Arc::new(fwd);
                    udp_forwarders
                        .lock()
                        .await
                        .insert(svc.remote_port, fwd.clone());
                    let svc_clone = svc.clone();
                    let out_clone = out_tx.clone();
                    let sd_rx = shutdown_tx.subscribe();
                    let rp = svc.remote_port;
                    tokio::spawn(async move {
                        fwd.run(svc_clone, rp, out_clone, sd_rx).await;
                    });
                }
            }

            if svc.effective_type() == "static" {
                let static_out = out_tx.clone();
                let static_name = svc.name.clone();
                let static_root = svc.local_addr.clone();
                let static_shutdown = shutdown_tx.subscribe();
                tokio::spawn(async move {
                    run_static_health_check(static_name, static_root, static_out, static_shutdown)
                        .await;
                });
            } else {
                if let Some(hc) = crate::health_check::HealthCheckConfig::from_service(svc) {
                    let hc_out = out_tx.clone();
                    let hc_name = svc.name.clone();
                    let hc_addr = svc.local_addr.clone();
                    let hc_sd = shutdown_tx.subscribe();
                    tokio::spawn(async move {
                        crate::health_check::run_health_check(hc_name, hc_addr, hc, hc_out, hc_sd)
                            .await;
                    });
                }
            }
        }

        for vcfg in &cfg.visitors {
            let v = vcfg.clone();
            let tls_cfg = self.tls_config.clone();
            let srv = server_addr.clone();
            let cid = client_id.clone();
            let auth = auth_key.clone();
            let sd = shutdown_tx.subscribe();
            tokio::spawn(async move {
                if let Err(e) = crate::visitor::run_visitor(v, tls_cfg, &srv, &cid, &auth, sd).await
                {
                    tracing::warn!("visitor error: {e}");
                }
            });
        }

        loop {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    let _ = shutdown_tx.send(());
                    return Ok(());
                }
                _ = keepalive.tick() => {
                    if last_pong.elapsed() > pong_timeout {
                        tracing::warn!("pong timeout, reconnecting");
                        let _ = shutdown_tx.send(());
                        return Err(anyhow!("keepalive timeout"));
                    }
                    let _ = out_tx.send(Message::new_ping());
                }
                msg = read_ndjson(&mut rd) => {
                    let msg = match msg {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::debug!("read error: {e}");
                            let _ = shutdown_tx.send(());
                            return Err(e);
                        }
                    };
                    last_pong = Instant::now();

                    match msg.ty.as_str() {
                        "ping" => {
                            let _ = out_tx.send(Message::new_pong());
                        }
                        "pong" => {}
                        "error" => {
                            let err = msg.error.as_deref().unwrap_or("unknown");
                            tracing::warn!("server error: {err}");
                            if err.contains("auth") || err.contains("fatal") {
                                let _ = shutdown_tx.send(());
                                return Err(anyhow!("server error: {err}"));
                            }
                        }
                        "open" => {
                            let tunnel_id = msg.id.clone().unwrap_or_default();
                            let address = msg.address.clone().unwrap_or_default();
                            let service = msg.service.clone();
                            let tls_cfg = self.tls_config.clone();
                            let srv = server_addr.clone();
                            let srv_name = server_name.clone();
                            let auth = auth_key.clone();
                            let cid = client_id.clone();
                            let qtp = srv_qcp_tunnel_port;
                            let mode = transport_mode;
                            let cache = transport_cache.clone();
                            let cache_key = transport_cache_key.clone();
                            let probe_timeout = qcp_probe_timeout;
                            let ws = self.web_state.clone();
                            let svc_name = service.as_ref().map(|s| s.name.clone()).unwrap_or_default();

                            tokio::spawn(async move {
                                if let Some(svc) = service {
                                    let res = handle_open(
                                        &tunnel_id, &address, &svc,
                                        tls_cfg, &srv, &srv_name, &auth, &cid,
                                        mode, qtp, cache, &cache_key, probe_timeout, ws,
                                    ).await;
                                    if let Err(e) = res {
                                        tracing::debug!(tunnel_id = %tunnel_id, service = %svc_name, "tunnel error: {e}");
                                    }
                                }
                            });
                        }
                        "udp_response" => {
                            if let (Some(rp), Some(addr), Some(data)) = (msg.remote_port, msg.address.as_ref(), msg.data.as_ref()) {
                                crate::udp::handle_udp_response(&udp_forwarders, rp, addr, data).await;
                            }
                        }
                        "bye" => {
                            tracing::info!("server sent bye");
                            let _ = shutdown_tx.send(());
                            return Ok(());
                        }
                        "close" => {
                            let reason = msg.error.as_deref().unwrap_or("server closed connection");
                            tracing::warn!("server closed: {reason}");
                            let _ = shutdown_tx.send(());
                            return Ok(());
                        }
                        _ => {
                            tracing::debug!(ty = %msg.ty, "unknown message type");
                        }
                    }
                }
            }
        }
    }
}

async fn run_static_health_check(
    service_name: String,
    root: String,
    out_tx: mpsc::UnboundedSender<Message>,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    // Older servers expect every active service to refresh a health timestamp.
    // A static service has no local TCP socket, so report directory availability
    // directly and keep it usable with both new and old servers.
    let mut ticker = interval(Duration::from_secs(20));
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => break,
            _ = ticker.tick() => {
                let healthy = static_root_is_available(&root);
                if out_tx.send(Message::new_health(&service_name, healthy)).is_err() {
                    break;
                }
            }
        }
    }
}

fn static_root_is_available(root: &str) -> bool {
    std::fs::canonicalize(root)
        .map(|path| path.is_dir())
        .unwrap_or(false)
}

async fn write_ndjson<W: AsyncWrite + Unpin>(w: &mut W, msg: &Message) -> Result<()> {
    let line = serde_json::to_string(msg)?;
    w.write_all(line.as_bytes()).await?;
    w.write_all(b"\n").await?;
    w.flush().await?;
    Ok(())
}

async fn read_ndjson<R: AsyncBufReadExt + Unpin>(r: &mut R) -> Result<Message> {
    let mut line = String::new();
    let n = r.read_line(&mut line).await?;
    if n == 0 {
        return Err(anyhow!("connection closed"));
    }
    let msg: Message = serde_json::from_str(line.trim())
        .with_context(|| format!("invalid message: {}", line.trim()))?;
    Ok(msg)
}

#[allow(clippy::too_many_arguments)]
async fn handle_open(
    tunnel_id: &str,
    remote_address: &str,
    service: &Service,
    tls_config: Arc<rustls::ClientConfig>,
    server_addr: &str,
    server_name: &str,
    auth_key: &str,
    client_id: &str,
    transport_mode: TransportMode,
    qcp_tunnel_port: u16,
    transport_cache: Arc<TransportCache>,
    transport_cache_key: &str,
    qcp_probe_timeout: Duration,
    web_state: WebState,
) -> Result<()> {
    if service.effective_type() == "static" {
        let tunnel = connect_tunnel_auto(
            transport_mode,
            &transport_cache,
            transport_cache_key,
            qcp_probe_timeout,
            server_addr,
            qcp_tunnel_port,
            tls_config,
            server_name,
            auth_key,
            client_id,
            tunnel_id,
            &web_state,
        )
        .await?;

        {
            let mut s = web_state.write().await;
            s.tunnels_active += 1;
        }

        let result = crate::static_files::serve(tunnel, &service.local_addr).await;

        {
            let mut s = web_state.write().await;
            s.tunnels_active = s.tunnels_active.saturating_sub(1);
        }
        return result;
    }

    let local_address = &service.local_addr;

    let mut local = TcpStream::connect(local_address)
        .await
        .with_context(|| format!("connect local {local_address}"))?;

    if service.proxy_protocol {
        let la = local.local_addr()?;
        let ra = remote_address.parse::<SocketAddr>().unwrap_or(la);
        let header =
            crate::proxy_protocol::build_header(ra, la, service.proxy_protocol_version.as_deref());
        local.write_all(&header).await?;
    }

    let mut tunnel = connect_tunnel_auto(
        transport_mode,
        &transport_cache,
        transport_cache_key,
        qcp_probe_timeout,
        server_addr,
        qcp_tunnel_port,
        tls_config,
        server_name,
        auth_key,
        client_id,
        tunnel_id,
        &web_state,
    )
    .await?;

    {
        let mut s = web_state.write().await;
        s.tunnels_active += 1;
    }

    let (n_in, n_out) = tokio::io::copy_bidirectional(&mut local, &mut tunnel).await?;
    {
        let mut s = web_state.write().await;
        s.bytes_in += n_in;
        s.bytes_out += n_out;
        s.tunnels_active = s.tunnels_active.saturating_sub(1);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn connect_tunnel_auto(
    mode: TransportMode,
    cache: &TransportCache,
    cache_key: &str,
    qcp_probe_timeout: Duration,
    server_addr: &str,
    qcp_tunnel_port: u16,
    tls_config: Arc<rustls::ClientConfig>,
    server_name: &str,
    auth_key: &str,
    client_id: &str,
    tunnel_id: &str,
    web_state: &WebState,
) -> Result<tunnel::TunnelStream> {
    let mut errors = Vec::new();
    for choice in cache.choices(mode, cache_key) {
        tracing::debug!(
            tunnel_id = %tunnel_id,
            transport = %choice.kind,
            source = choice.source.as_str(),
            "opening tunnel"
        );
        let result = if choice.kind == TransportKind::Qcp {
            match tokio::time::timeout(
                qcp_probe_timeout,
                tunnel::connect_tunnel(
                    choice.kind,
                    server_addr,
                    qcp_tunnel_port,
                    tls_config.clone(),
                    server_name,
                    auth_key,
                    client_id,
                    tunnel_id,
                ),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(anyhow!(
                    "QCP tunnel setup exceeded {} ms",
                    qcp_probe_timeout.as_millis()
                )),
            }
        } else {
            connect_tcp_tunnel_with_retries(
                server_addr,
                tls_config.clone(),
                server_name,
                auth_key,
                client_id,
                tunnel_id,
            )
            .await
        };

        match result {
            Ok(tunnel) => {
                if mode.is_auto() {
                    if let Err(error) = cache.note_success(cache_key, choice.kind) {
                        tracing::warn!(path = %cache.path().display(), "cannot update transport cache: {error}");
                    }
                }
                {
                    let mut state = web_state.write().await;
                    state.active_tunnel_transport = choice.kind.as_str().to_string();
                }
                tracing::debug!(tunnel_id = %tunnel_id, transport = %choice.kind, "tunnel opened");
                return Ok(tunnel);
            }
            Err(error) => {
                tracing::debug!(
                    tunnel_id = %tunnel_id,
                    transport = %choice.kind,
                    source = choice.source.as_str(),
                    "tunnel setup failed: {error:#}"
                );
                if mode.is_auto() {
                    if let Err(cache_error) = cache.note_failure(cache_key, choice.kind, &error) {
                        tracing::warn!(path = %cache.path().display(), "cannot update transport cache: {cache_error}");
                    }
                }
                errors.push(format!("{}: {error:#}", choice.kind));
            }
        }
    }
    Err(anyhow!(
        "all tunnel transports failed ({})",
        errors.join("; ")
    ))
}

async fn connect_tcp_tunnel_with_retries(
    server_addr: &str,
    tls_config: Arc<rustls::ClientConfig>,
    server_name: &str,
    auth_key: &str,
    client_id: &str,
    tunnel_id: &str,
) -> Result<tunnel::TunnelStream> {
    let mut errors = Vec::with_capacity(TCP_TUNNEL_ATTEMPTS as usize);

    for attempt in 1..=TCP_TUNNEL_ATTEMPTS {
        let result = timeout(
            TCP_TUNNEL_ATTEMPT_TIMEOUT,
            tunnel::connect_tunnel(
                TransportKind::Tcp,
                server_addr,
                0,
                tls_config.clone(),
                server_name,
                auth_key,
                client_id,
                tunnel_id,
            ),
        )
        .await;

        match result {
            Ok(Ok(tunnel)) => {
                if attempt > 1 {
                    tracing::debug!(
                        tunnel_id = %tunnel_id,
                        attempt,
                        "TCP tunnel retry recovered"
                    );
                }
                return Ok(tunnel);
            }
            Ok(Err(error)) => errors.push(format!("attempt {attempt}: {error:#}")),
            Err(_) => errors.push(format!(
                "attempt {attempt}: setup exceeded {} ms",
                TCP_TUNNEL_ATTEMPT_TIMEOUT.as_millis()
            )),
        }

        if attempt < TCP_TUNNEL_ATTEMPTS {
            tracing::debug!(
                tunnel_id = %tunnel_id,
                attempt,
                total_attempts = TCP_TUNNEL_ATTEMPTS,
                "TCP tunnel attempt failed; retrying"
            );
            sleep(TCP_TUNNEL_RETRY_DELAY).await;
        }
    }

    Err(anyhow!(
        "TCP tunnel setup failed after {} attempts ({})",
        TCP_TUNNEL_ATTEMPTS,
        errors.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cross233_protocol::tls::{
        acceptor, client_config, gen_self_signed, gen_self_signed_pem, server_config_pem,
    };
    use tokio::io::{AsyncBufReadExt, BufReader, BufStream};
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    #[test]
    fn tls_requires_ca_unless_insecure_is_explicit() {
        let cfg = ClientConfig::default();
        assert!(build_tls_config(&cfg).is_err());

        let mut insecure = cfg;
        insecure.insecure = true;
        assert!(build_tls_config(&insecure).is_ok());
    }

    #[test]
    fn static_health_requires_an_existing_directory() {
        let root = std::env::temp_dir().join(format!(
            "cross233-static-health-test-{}",
            cross233_protocol::random_id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        assert!(super::static_root_is_available(
            root.to_string_lossy().as_ref()
        ));

        let file = root.join("file.txt");
        std::fs::write(&file, "test").unwrap();
        assert!(!super::static_root_is_available(
            file.to_string_lossy().as_ref()
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn tcp_tunnel_retries_after_a_transient_tls_eof() {
        let (cert, key) = gen_self_signed();
        let acceptor = acceptor(&cert, &key);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            // Simulate an upstream accepting TCP but dropping the first TLS
            // handshake.  The next sequential attempt must reuse the id.
            let (first, _) = listener.accept().await.unwrap();
            drop(first);

            let (second, _) = listener.accept().await.unwrap();
            let tls = acceptor.accept(second).await.unwrap();
            let mut stream = BufStream::new(tls);
            let hello = tunnel::read_ndjson(&mut stream).await.unwrap();
            assert_eq!(hello.ty, "tunnel");
            assert_eq!(hello.id.as_deref(), Some("retry-id"));

            tunnel::write_ndjson(&mut stream, &Message::new_challenge("test-nonce"))
                .await
                .unwrap();
            let auth = tunnel::read_ndjson(&mut stream).await.unwrap();
            assert_eq!(auth.ty, "auth");
            tunnel::write_ndjson(&mut stream, &Message::new_ready(Vec::new(), 0, 0))
                .await
                .unwrap();
        });

        let tunnel = super::connect_tcp_tunnel_with_retries(
            &addr.to_string(),
            Arc::new(client_config(&cert)),
            "localhost",
            "test-auth-key",
            "client-id",
            "retry-id",
        )
        .await
        .expect("second TCP attempt should authenticate");
        drop(tunnel);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn session_registers_service_over_tls() {
        let (cert_pem, key_pem) = gen_self_signed_pem();
        let cert_path = std::env::temp_dir().join(format!(
            "cross233-client-session-{}.pem",
            cross233_protocol::random_id()
        ));
        std::fs::write(&cert_path, &cert_pem).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_config_pem(
            cert_pem.as_bytes(),
            key_pem.as_bytes(),
        )));

        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let tls = acceptor.accept(tcp).await.unwrap();
            let mut stream = BufReader::new(tls);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let hello: Message = serde_json::from_str(line.trim()).unwrap();
            super::write_ndjson(&mut stream, &Message::new_challenge("test-nonce"))
                .await
                .unwrap();

            line.clear();
            stream.read_line(&mut line).await.unwrap();
            let auth: Message = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(auth.ty, "auth");
            super::write_ndjson(
                &mut stream,
                &Message::new_ready(hello.services.clone(), 17713, 17714),
            )
            .await
            .unwrap();
            hello
        });

        let cfg = ClientConfig {
            server: addr.to_string(),
            server_name: "localhost".to_string(),
            auth_key: "test-key".to_string(),
            ca_file: cert_path.to_string_lossy().to_string(),
            client_id: "session-test".to_string(),
            services: vec![Service {
                name: "local-http".to_string(),
                ty: Some("tcp".to_string()),
                local_addr: "127.0.0.1:80".to_string(),
                remote_port: 60080,
                ..Default::default()
            }],
            ..Default::default()
        };

        let web_state = crate::web::new_state();
        let client =
            Client::new(cfg, web_state.clone(), Arc::new(tokio::sync::Notify::new())).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), client.session()).await;
        let hello = server.await.unwrap();
        std::fs::remove_file(&cert_path).unwrap();

        assert!(result.is_ok(), "client session timed out");
        assert!(result.unwrap().is_err(), "server close should end session");
        assert_eq!(hello.ty, "client");
        assert_eq!(hello.client_id.as_deref(), Some("session-test"));
        assert!(web_state.read().await.connected);
    }
}
