use anyhow::{anyhow, Context, Result};
use tokio::net::lookup_host;

/// Create a reliable UDP stream for one TLS-protected tunnel.
///
/// The returned stream has no application authentication of its own: callers
/// must run the normal TLS and tunnel HMAC handshake on top of it.
pub async fn connect_tunnel_stream(
    server_addr: &str,
    qcp_tunnel_port: u16,
) -> Result<cross233_qcp::QcpStream> {
    if qcp_tunnel_port == 0 {
        return Err(anyhow!("QCP tunnel transport is disabled by the server"));
    }
    let endpoint = resolve_endpoint(server_addr, qcp_tunnel_port).await?;
    let conn = cross233_qcp::dial(&endpoint)
        .await
        .with_context(|| format!("dial QCP tunnel {endpoint}"))?;
    Ok(cross233_qcp::into_stream(conn, cross233_qcp::Stream::Batch))
}

async fn resolve_endpoint(server_addr: &str, qcp_tunnel_port: u16) -> Result<String> {
    let host = server_host(server_addr)?;
    let mut addresses = lookup_host((host.as_str(), qcp_tunnel_port))
        .await
        .with_context(|| format!("resolve QCP server {host}:{qcp_tunnel_port}"))?;
    addresses
        .next()
        .map(|addr| addr.to_string())
        .ok_or_else(|| anyhow!("no addresses found for QCP server {host}:{qcp_tunnel_port}"))
}

fn server_host(server_addr: &str) -> Result<String> {
    if let Some(rest) = server_addr.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| anyhow!("invalid bracketed server address: {server_addr}"))?;
        if !rest[end + 1..].starts_with(':') {
            return Err(anyhow!("server address must include a port: {server_addr}"));
        }
        return Ok(rest[..end].to_string());
    }
    let (host, port) = server_addr
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("server address must include a port: {server_addr}"))?;
    if host.is_empty() || port.is_empty() {
        return Err(anyhow!("invalid server address: {server_addr}"));
    }
    Ok(host.to_string())
}

#[cfg(test)]
mod tests {
    use super::server_host;

    #[test]
    fn extracts_ipv4_hostname_and_ipv6_hosts() {
        assert_eq!(server_host("127.0.0.1:7710").unwrap(), "127.0.0.1");
        assert_eq!(
            server_host("server.example:7710").unwrap(),
            "server.example"
        );
        assert_eq!(server_host("[2001:db8::1]:7710").unwrap(), "2001:db8::1");
        assert!(server_host("bad-address").is_err());
    }
}
