use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use std::io::{BufReader, Cursor};
use std::path::Path;

type HmacSha256 = Hmac<Sha256>;

fn restrict_private_file_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

pub fn generate_nonce() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn compute_proof(key: &str, ty: &str, client_id: &str, id: &str, nonce: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let msg = format!("cross233/v1\x00{ty}\x00{client_id}\x00{id}\x00{nonce}");
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("hmac init");
    mac.update(msg.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

pub fn verify_proof(
    key: &str,
    ty: &str,
    client_id: &str,
    id: &str,
    nonce: &str,
    proof: &str,
) -> bool {
    let expected = compute_proof(key, ty, client_id, id, nonce);
    expected == proof
}

pub fn load_or_create_cert(
    cert_path: &Path,
    key_path: &Path,
) -> anyhow::Result<(rustls::ServerConfig, String)> {
    if cert_path.exists() && key_path.exists() {
        return load_cert(cert_path, key_path);
    }

    tracing::info!(cert=?cert_path, key=?key_path, "cert files not found, generating self-signed cert");
    let cert = rcgen::generate_simple_self_signed(vec!["cross233".to_string()])?;
    let cert_pem = cert.cert.pem();
    let key_pem = cert.key_pair.serialize_pem();

    if let Some(parent) = cert_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(cert_path, &cert_pem)?;
    std::fs::write(key_path, &key_pem)?;
    restrict_private_file_permissions(key_path)?;

    load_cert(cert_path, key_path)
}

fn load_cert(cert_path: &Path, key_path: &Path) -> anyhow::Result<(rustls::ServerConfig, String)> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let cert_pem = std::fs::read(cert_path)?;
    let key_pem = std::fs::read(key_path)?;

    let mut cert_reader = BufReader::new(Cursor::new(&cert_pem));
    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        anyhow::bail!("no certificates found in {:?}", cert_path);
    }

    let mut key_reader = BufReader::new(Cursor::new(&key_pem));
    let key = rustls_pemfile::private_key(&mut key_reader)?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {:?}", key_path))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    let fingerprint = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&cert_pem);
        let digest = h.finalize();
        hex::encode(digest)
    };

    Ok((config, fingerprint))
}

pub fn load_or_create_auth_key(path: &Path, provided: &str) -> anyhow::Result<String> {
    if !provided.is_empty() {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::write(path, provided)?;
            restrict_private_file_permissions(path)?;
        }
        return Ok(provided.to_string());
    }
    if path.exists() {
        let k = std::fs::read_to_string(path)?.trim().to_string();
        if !k.is_empty() {
            return Ok(k);
        }
    }
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let k = hex::encode(bytes);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, &k)?;
    restrict_private_file_permissions(path)?;
    tracing::info!(key_file=?path, "generated new auth key");
    Ok(k)
}

pub fn cidr_contains(cidrs: &[String], ip: std::net::IpAddr) -> bool {
    for c in cidrs {
        if let Ok(net) = c.parse::<ipnet::IpNet>() {
            if net.contains(&ip) {
                return true;
            }
        }
    }
    false
}
