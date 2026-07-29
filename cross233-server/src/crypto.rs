use anyhow::{Context, Result};
use cross233_protocol::crypto::load_or_create_cert;
use rustls::ServerConfig;
use std::path::Path;

pub use cross233_protocol::crypto::{cidr_contains, compute_proof, generate_nonce, verify_proof};

pub struct TlsSetup {
    pub server_config: ServerConfig,
    pub fingerprint: String,
}

pub fn setup_tls(cert_file: &str, key_file: &str) -> Result<TlsSetup> {
    let cert_path = Path::new(cert_file);
    let key_path = Path::new(key_file);
    let (server_config, fingerprint) =
        load_or_create_cert(cert_path, key_path).with_context(|| {
            format!(
                "failed to load/create cert at {} and key at {}",
                cert_file, key_file
            )
        })?;
    Ok(TlsSetup {
        server_config,
        fingerprint,
    })
}

pub fn load_or_create_auth_key(path: &str, provided: &str) -> Result<String> {
    cross233_protocol::crypto::load_or_create_auth_key(Path::new(path), provided)
        .with_context(|| format!("failed to load/create auth key at {}", path))
}
