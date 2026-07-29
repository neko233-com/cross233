use anyhow::{anyhow, Context, Result};
use cross233_protocol::Message;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::{TlsConnector, TlsStream};

use crate::config::VisitorConfig;
use crate::tunnel;

pub async fn run_visitor(
    cfg: VisitorConfig,
    tls_config: Arc<rustls::ClientConfig>,
    server_addr: &str,
    client_id: &str,
    auth_key: &str,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) -> Result<()> {
    match cfg.ty.as_str() {
        "stcp" => {
            run_stcp_visitor(
                cfg,
                tls_config,
                server_addr,
                client_id,
                auth_key,
                shutdown_rx,
            )
            .await
        }
        other => Err(anyhow!("unsupported visitor type: {other}")),
    }
}

async fn run_stcp_visitor(
    cfg: VisitorConfig,
    tls_config: Arc<rustls::ClientConfig>,
    server_addr: &str,
    client_id: &str,
    auth_key: &str,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) -> Result<()> {
    let bind_addr = format!("{}:{}", cfg.bind_addr, cfg.bind_port);
    let listener = TcpListener::bind(&bind_addr).await?;
    tracing::info!(service = %cfg.name, bind = %bind_addr, "stcp visitor listening");

    let server_name = cfg.server_name.clone();
    let client_id = client_id.to_string();
    let auth_key = auth_key.to_string();
    let server_addr = server_addr.to_string();
    let name = cfg.name.clone();

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => break,
            res = listener.accept() => {
                match res {
                    Ok((local, _)) => {
                        let tls_config = tls_config.clone();
                        let server_addr = server_addr.clone();
                        let server_name = server_name.clone();
                        let client_id = client_id.clone();
                        let auth_key = auth_key.clone();
                        let name = name.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_stcp_conn(
                                local, tls_config, &server_addr, &server_name,
                                &client_id, &auth_key, &name,
                            ).await {
                                tracing::debug!("stcp visitor conn error: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("stcp visitor accept error: {e}");
                    }
                }
            }
        }
    }
    Ok(())
}

async fn handle_stcp_conn(
    mut local: TcpStream,
    tls_config: Arc<rustls::ClientConfig>,
    server_addr: &str,
    server_name: &str,
    client_id: &str,
    auth_key: &str,
    service_name: &str,
) -> Result<()> {
    let tcp = TcpStream::connect(server_addr)
        .await
        .with_context(|| format!("visitor connect {server_addr}"))?;
    tcp.set_nodelay(true)?;
    let connector = TlsConnector::from(tls_config);
    let name =
        ServerName::try_from(server_name.to_owned()).map_err(|_| anyhow!("invalid server name"))?;
    let mut remote = connector.connect(name, tcp).await?;

    let visitor_id = cross233_protocol::random_id();
    let hello = Message::new_visitor_hello(client_id, service_name, Some(&visitor_id));
    tunnel::write_ndjson(&mut remote, &hello).await?;

    let challenge = tunnel::read_ndjson(&mut remote).await?;
    if challenge.ty != "challenge" {
        return Err(anyhow!("expected challenge"));
    }
    let nonce = challenge.nonce.as_deref().unwrap_or("");
    let proof = crate::auth::compute_proof(auth_key, "visitor", client_id, &visitor_id, nonce);
    tunnel::write_ndjson(&mut remote, &Message::new_auth(&proof)).await?;

    let resp = tunnel::read_ndjson(&mut remote).await?;
    if resp.ty == "error" {
        return Err(anyhow!(
            "visitor auth failed: {}",
            resp.error.as_deref().unwrap_or("")
        ));
    }
    if resp.ty != "ready" && resp.ty != "visitor_ok" {
        return Err(anyhow!("expected ready after visitor auth"));
    }

    let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
    Ok(())
}

pub struct SudpVisitor {
    sock: tokio::net::UdpSocket,
    out_tx: mpsc::UnboundedSender<Message>,
}

impl SudpVisitor {
    pub async fn new(bind_addr: &str) -> Result<(Self, mpsc::UnboundedReceiver<Message>)> {
        let sock = tokio::net::UdpSocket::bind(bind_addr).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        Ok((Self { sock, out_tx: tx }, rx))
    }
}
