use anyhow::{anyhow, Context, Result};
use cross233_protocol::Message;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

use crate::transport::TransportKind;

const MAX_NDJSON_BYTES: usize = 16 * 1024;

pub trait TunnelIo: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> TunnelIo for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

pub type TunnelStream = Box<dyn TunnelIo>;

pub async fn connect_tunnel(
    transport: TransportKind,
    server_addr: &str,
    qcp_tunnel_port: u16,
    tls_config: Arc<rustls::ClientConfig>,
    server_name: &str,
    auth_key: &str,
    _client_id: &str,
    tunnel_id: &str,
) -> Result<TunnelStream> {
    match transport {
        TransportKind::Tcp => {
            let tcp = TcpStream::connect(server_addr)
                .await
                .with_context(|| format!("tunnel TCP connect to {server_addr}"))?;
            tcp.set_nodelay(true)?;
            let connector = TlsConnector::from(tls_config);
            let name = tls_server_name(server_name)?;
            let mut tls = connector
                .connect(name, tcp)
                .await
                .context("tunnel TCP TLS handshake")?;
            authenticate_tunnel(&mut tls, auth_key, tunnel_id).await?;
            Ok(Box::new(tls))
        }
        TransportKind::Qcp => {
            let qcp = crate::qcp::connect_tunnel_stream(server_addr, qcp_tunnel_port).await?;
            let connector = TlsConnector::from(tls_config);
            let name = tls_server_name(server_name)?;
            let mut tls = connector
                .connect(name, qcp)
                .await
                .context("tunnel QCP TLS handshake")?;
            authenticate_tunnel(&mut tls, auth_key, tunnel_id).await?;
            Ok(Box::new(tls))
        }
    }
}

async fn authenticate_tunnel<S>(stream: &mut S, auth_key: &str, tunnel_id: &str) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_ndjson(stream, &Message::new_tunnel_hello(tunnel_id)).await?;

    let challenge = read_ndjson(stream).await?;
    if challenge.ty != "challenge" {
        return Err(anyhow!("expected challenge, got {}", challenge.ty));
    }
    let nonce = challenge.nonce.as_deref().unwrap_or("");

    let proof = crate::auth::compute_proof(auth_key, "tunnel", "", tunnel_id, nonce);
    write_ndjson(stream, &Message::new_auth(&proof)).await?;

    let resp = read_ndjson(stream).await?;
    if resp.ty == "error" {
        return Err(anyhow!(
            "tunnel auth rejected: {}",
            resp.error.as_deref().unwrap_or("unknown")
        ));
    }
    if resp.ty != "ready" && resp.ty != "tunnel_ok" {
        return Err(anyhow!("expected ready, got {}", resp.ty));
    }
    Ok(())
}

fn tls_server_name(server_name: &str) -> Result<ServerName<'static>> {
    ServerName::try_from(server_name.to_owned())
        .map_err(|_| anyhow!("invalid server name: {server_name}"))
}

pub async fn write_ndjson<W: AsyncWrite + Unpin>(w: &mut W, msg: &Message) -> Result<()> {
    let line = serde_json::to_string(msg)?;
    w.write_all(line.as_bytes()).await?;
    w.write_all(b"\n").await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_ndjson<R: AsyncRead + Unpin>(r: &mut R) -> Result<Message> {
    let mut line = Vec::new();
    loop {
        let byte = r.read_u8().await?;
        if byte == b'\n' {
            break;
        }
        line.push(byte);
        if line.len() > MAX_NDJSON_BYTES {
            return Err(anyhow!("NDJSON message exceeds {MAX_NDJSON_BYTES} bytes"));
        }
    }
    let s = String::from_utf8_lossy(&line);
    let msg: Message =
        serde_json::from_str(s.trim()).with_context(|| format!("invalid message: {}", s.trim()))?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cross233_protocol::tls::{acceptor, connector, gen_self_signed};
    use cross233_protocol::Service;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufStream};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::rustls::pki_types::ServerName;

    #[tokio::test]
    async fn ndjson_client_hello_reaches_tls_server() {
        let (cert, key) = gen_self_signed();
        let acceptor = acceptor(&cert, &key);
        let connector = connector(&cert);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let tls = acceptor.accept(tcp).await.unwrap();
            let mut stream = BufStream::new(tls);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            serde_json::from_str::<Message>(line.trim()).unwrap()
        });

        let tcp = TcpStream::connect(addr).await.unwrap();
        let name = ServerName::try_from("localhost".to_owned()).unwrap();
        let mut tls = connector.connect(name, tcp).await.unwrap();
        let message = Message::new_client_hello(
            "test-client",
            vec![Service {
                name: "local-http".to_string(),
                ty: Some("tcp".to_string()),
                local_addr: "127.0.0.1:80".to_string(),
                remote_port: 60080,
                ..Default::default()
            }],
        );
        write_ndjson(&mut tls, &message).await.unwrap();

        let received = server.await.unwrap();
        assert_eq!(received.ty, "client");
        assert_eq!(received.client_id.as_deref(), Some("test-client"));
        assert_eq!(received.services.len(), 1);
        assert_eq!(received.services[0].name, "local-http");
        assert_eq!(received.services[0].local_addr, "127.0.0.1:80");
        assert_eq!(received.services[0].remote_port, 60080);
    }

    #[tokio::test]
    async fn tls_round_trip_over_qcp() {
        let (cert, key) = gen_self_signed();
        let acceptor = acceptor(&cert, &key);
        let connector = connector(&cert);
        let mut listener = cross233_qcp::listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().to_string();

        let server = tokio::spawn(async move {
            let conn = listener.accept().await.unwrap();
            let qcp = cross233_qcp::into_stream(conn, cross233_qcp::Stream::Batch);
            let mut tls = acceptor.accept(qcp).await.unwrap();
            let mut request = [0_u8; 5];
            tls.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"hello");
            tls.write_all(b"ready").await.unwrap();
            tls.flush().await.unwrap();
        });

        let conn = cross233_qcp::dial(&addr).await.unwrap();
        let qcp = cross233_qcp::into_stream(conn, cross233_qcp::Stream::Batch);
        let name = ServerName::try_from("localhost".to_owned()).unwrap();
        let mut tls = connector.connect(name, qcp).await.unwrap();
        tls.write_all(b"hello").await.unwrap();
        tls.flush().await.unwrap();
        let mut response = [0_u8; 5];
        tls.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"ready");
        server.await.unwrap();
    }
}
