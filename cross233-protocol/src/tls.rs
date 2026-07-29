//! TLS 1.3 configuration helpers for the cross233 control channel.
//!
//! The control connection is always TLS 1.3. Authentication is layered on top
//! via HMAC (see [`crate::auth`]), but the transport is mutually authenticated
//! at the TLS layer too: the server presents a certificate and the client
//! verifies it against a pinned/trusted root. For internal deployments a
//! self-signed cert is fine; [`gen_self_signed`] generates one for demos/tests.

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ClientConfig as RustlsClientConfig;
use rustls::RootCertStore;
use rustls::ServerConfig as RustlsServerConfig;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::TlsConnector;

/// Generate a self-signed cert (CN=localhost, SAN=127.0.0.1) as DER bytes.
/// Returns `(cert_der, key_der)`.
pub fn gen_self_signed() -> (Vec<u8>, Vec<u8>) {
    let ck = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "cross233".to_string(),
    ])
    .expect("generate self-signed cert");
    (ck.cert.der().to_vec(), ck.key_pair.serialize_der())
}

/// Generate a self-signed cert as PEM strings `(cert_pem, key_pem)`.
pub fn gen_self_signed_pem() -> (String, String) {
    let ck = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "cross233".to_string(),
    ])
    .expect("generate self-signed cert");
    (ck.cert.pem(), ck.key_pair.serialize_pem())
}

/// Build a `rustls::ServerConfig` from DER cert + key (PKCS#8).
pub fn server_config(cert_der: &[u8], key_der: &[u8]) -> RustlsServerConfig {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let cert = CertificateDer::from(cert_der.to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der.to_vec()));
    RustlsServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("valid server config")
}

/// Build a `rustls::ClientConfig` that trusts `trusted_cert_der` as its only root.
pub fn client_config(trusted_cert_der: &[u8]) -> RustlsClientConfig {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(trusted_cert_der.to_vec()))
        .expect("add trusted root");
    RustlsClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// Build a TLS acceptor (server side) from DER cert + key.
pub fn acceptor(cert_der: &[u8], key_der: &[u8]) -> TlsAcceptor {
    TlsAcceptor::from(std::sync::Arc::new(server_config(cert_der, key_der)))
}

/// Build a TLS connector (client side) trusting `trusted_cert_der`.
pub fn connector(trusted_cert_der: &[u8]) -> TlsConnector {
    TlsConnector::from(std::sync::Arc::new(client_config(trusted_cert_der)))
}

/// Build a server config from PEM cert + key files' contents.
pub fn server_config_pem(mut cert_pem: &[u8], mut key_pem: &[u8]) -> RustlsServerConfig {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem)
        .collect::<Result<_, _>>()
        .expect("parse server cert pem");
    let key = rustls_pemfile::private_key(&mut key_pem)
        .expect("parse server key pem")
        .expect("server key present");
    RustlsServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("valid server config")
}

/// Build a client config trusting a PEM cert as root.
pub fn client_config_pem(mut trusted_pem: &[u8]) -> RustlsClientConfig {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let mut roots = RootCertStore::empty();
    for c in rustls_pemfile::certs(&mut trusted_pem)
        .collect::<Result<Vec<_>, _>>()
        .expect("parse trusted pem")
    {
        roots.add(c).expect("add root");
    }
    RustlsClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// Parse PEM cert + key files into DER byte vectors for [`server_config`].
pub fn load_cert_key_der(mut cert_pem: &[u8], mut key_pem: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem)
        .collect::<Result<_, _>>()
        .expect("parse server cert pem");
    let key = rustls_pemfile::private_key(&mut key_pem)
        .expect("parse server key pem")
        .expect("server key present");
    (
        certs
            .into_iter()
            .next()
            .expect("at least one cert")
            .as_ref()
            .to_vec(),
        key.secret_der().to_vec(),
    )
}

/// Extract the (first) certificate DER from a PEM trust file.
pub fn load_trusted_cert_der(mut pem: &[u8]) -> Vec<u8> {
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut pem)
        .collect::<Result<_, _>>()
        .expect("parse trusted cert pem");
    certs
        .into_iter()
        .next()
        .expect("at least one cert")
        .as_ref()
        .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufStream};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::rustls::pki_types::ServerName;

    #[tokio::test]
    async fn tls_stream_round_trips_application_data() {
        let (cert, key) = gen_self_signed();
        let acceptor = acceptor(&cert, &key);
        let connector = connector(&cert);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let tls = acceptor.accept(tcp).await.unwrap();
            let mut tls = BufStream::new(tls);
            let mut line = String::new();
            tls.read_line(&mut line).await.unwrap();
            assert_eq!(line, "hello\n");
            tls.write_all(b"ready\n").await.unwrap();
            tls.flush().await.unwrap();
        });

        let tcp = TcpStream::connect(addr).await.unwrap();
        let name = ServerName::try_from("localhost".to_owned()).unwrap();
        let mut tls = connector.connect(name, tcp).await.unwrap();
        tls.write_all(b"hello\n").await.unwrap();
        tls.flush().await.unwrap();
        let mut response = [0u8; 6];
        tokio::io::AsyncReadExt::read_exact(&mut tls, &mut response)
            .await
            .unwrap();
        assert_eq!(&response, b"ready\n");
        server.await.unwrap();
    }
}
