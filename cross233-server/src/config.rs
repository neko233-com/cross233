use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
    pub proxy_bind: String,
    pub auth_key: String,
    pub auth_key_file: String,
    pub cert_file: String,
    pub key_file: String,
    pub control_port: u16,
    pub web_port: u16,
    pub http_vhost_port: u16,
    pub https_vhost_port: u16,
    pub tcpmux_port: u16,
    pub qcp_port: u16,
    /// TLS-over-QCP tunnel listener. 0 disables it.
    pub qcp_tunnel_port: u16,
    pub subdomain_host: String,
    pub port_min: u16,
    pub port_max: u16,
    /// Permit direct TCP/UDP services below port 1024. Disabled by default so
    /// a client cannot accidentally shadow SSH or another system daemon.
    pub allow_privileged_ports: bool,
    /// Ports that clients may never claim directly, even when privileged
    /// ports are enabled. SSH is protected by default.
    pub protected_ports: Vec<u16>,
    pub max_connections: u32,
    pub handshake_timeout: u32,
    pub web_user: String,
    pub web_password: String,
    pub api_token: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0".to_string(),
            proxy_bind: String::new(),
            auth_key: String::new(),
            auth_key_file: "cross233-auth.key".to_string(),
            cert_file: "cross233-cert.pem".to_string(),
            key_file: "cross233-key.pem".to_string(),
            control_port: 7710,
            web_port: 7711,
            http_vhost_port: 80,
            https_vhost_port: 443,
            tcpmux_port: 0,
            qcp_port: 7713,
            qcp_tunnel_port: 0,
            subdomain_host: String::new(),
            port_min: 7712,
            port_max: 7720,
            allow_privileged_ports: false,
            protected_ports: vec![22],
            max_connections: 256,
            handshake_timeout: 10,
            web_user: String::new(),
            web_password: String::new(),
            api_token: String::new(),
        }
    }
}

impl ServerConfig {
    pub fn load(path: Option<&str>) -> Result<Self> {
        let mut config = Self::default();
        if let Some(p) = path {
            let path = Path::new(p);
            if path.exists() {
                let content = std::fs::read_to_string(path)
                    .with_context(|| format!("failed to read config file: {}", p))?;
                config = match path.extension().and_then(|e| e.to_str()) {
                    Some("toml") => toml::from_str(&content)
                        .with_context(|| format!("failed to parse TOML config: {}", p))?,
                    Some("json") => serde_json::from_str(&content)
                        .with_context(|| format!("failed to parse JSON config: {}", p))?,
                    Some("yaml") | Some("yml") => serde_yaml::from_str(&content)
                        .with_context(|| format!("failed to parse YAML config: {}", p))?,
                    _ => anyhow::bail!("unsupported config format: {}", p),
                };
            }
        }
        Ok(config)
    }

    pub fn control_addr(&self) -> String {
        format!("{}:{}", self.bind, self.control_port)
    }

    pub fn web_addr(&self) -> String {
        format!("{}:{}", self.bind, self.web_port)
    }

    pub fn http_vhost_addr(&self) -> String {
        format!("{}:{}", self.bind, self.http_vhost_port)
    }

    pub fn https_vhost_addr(&self) -> String {
        format!("{}:{}", self.bind, self.https_vhost_port)
    }

    pub fn tcpmux_addr(&self) -> Option<String> {
        if self.tcpmux_port > 0 {
            Some(format!("{}:{}", self.bind, self.tcpmux_port))
        } else {
            None
        }
    }

    pub fn qcp_addr(&self) -> String {
        format!("{}:{}", self.bind, self.qcp_port)
    }

    pub fn qcp_tunnel_addr(&self) -> Option<String> {
        if self.qcp_tunnel_port == 0 {
            None
        } else {
            Some(format!("{}:{}", self.bind, self.qcp_tunnel_port))
        }
    }

    pub fn effective_proxy_bind(&self) -> &str {
        if self.proxy_bind.trim().is_empty() {
            &self.bind
        } else {
            &self.proxy_bind
        }
    }

    /// SSH is always protected. `protected_ports` only adds more ports; it
    /// cannot remove this invariant.
    pub fn effective_protected_ports(&self) -> HashSet<u16> {
        let mut protected: HashSet<u16> = self.protected_ports.iter().copied().collect();
        protected.insert(22);
        protected
    }

    /// Ports owned by the server itself must never be assigned to a direct
    /// client service. Zero means disabled and is intentionally excluded.
    pub fn reserved_ports(&self) -> HashSet<u16> {
        [
            self.control_port,
            self.web_port,
            self.http_vhost_port,
            self.https_vhost_port,
            self.tcpmux_port,
            self.qcp_port,
            self.qcp_tunnel_port,
        ]
        .into_iter()
        .filter(|port| *port != 0)
        .collect()
    }

    pub fn validate(&self) -> Result<()> {
        self.bind
            .parse::<IpAddr>()
            .with_context(|| format!("bind must be an IP address: {}", self.bind))?;
        self.effective_proxy_bind()
            .parse::<IpAddr>()
            .with_context(|| {
                format!(
                    "proxy_bind must be an IP address: {}",
                    self.effective_proxy_bind()
                )
            })?;

        if self.control_port == 0 {
            anyhow::bail!("control_port must not be zero");
        }
        if self.port_min == 0 || self.port_max == 0 {
            anyhow::bail!("automatic proxy port range must not include port zero");
        }
        if self.port_min > self.port_max {
            anyhow::bail!(
                "invalid automatic proxy port range: {} is greater than {}",
                self.port_min,
                self.port_max
            );
        }

        validate_unique_ports(
            "TCP",
            &[
                ("control_port", self.control_port),
                ("web_port", self.web_port),
                ("http_vhost_port", self.http_vhost_port),
                ("https_vhost_port", self.https_vhost_port),
                ("tcpmux_port", self.tcpmux_port),
            ],
        )?;
        validate_unique_ports(
            "UDP",
            &[
                ("qcp_port", self.qcp_port),
                ("qcp_tunnel_port", self.qcp_tunnel_port),
            ],
        )?;

        let reserved = self.reserved_ports();
        let protected = self.effective_protected_ports();
        let has_safe_auto_port = (self.port_min..=self.port_max).any(|port| {
            !reserved.contains(&port)
                && !protected.contains(&port)
                && (self.allow_privileged_ports || port >= 1024)
        });
        if !has_safe_auto_port {
            anyhow::bail!(
                "automatic proxy port range {}-{} contains no assignable port",
                self.port_min,
                self.port_max
            );
        }

        Ok(())
    }
}

fn validate_unique_ports(protocol: &str, ports: &[(&str, u16)]) -> Result<()> {
    let mut seen = std::collections::HashMap::new();
    for (name, port) in ports.iter().copied().filter(|(_, port)| *port != 0) {
        if let Some(previous) = seen.insert(port, name) {
            anyhow::bail!(
                "{protocol} listener port conflict: {previous} and {name} both use {port}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ServerConfig;

    #[test]
    fn proxy_bind_falls_back_to_main_bind() {
        let mut config = ServerConfig {
            bind: "127.0.0.1".to_string(),
            ..ServerConfig::default()
        };
        assert_eq!(config.effective_proxy_bind(), "127.0.0.1");

        config.proxy_bind = "192.0.2.10".to_string();
        assert_eq!(config.effective_proxy_bind(), "192.0.2.10");
    }

    #[test]
    fn server_listener_ports_are_reserved() {
        let config = ServerConfig::default();
        let reserved = config.reserved_ports();
        assert!(reserved.contains(&config.control_port));
        assert!(reserved.contains(&config.http_vhost_port));
        assert!(!reserved.contains(&0));
    }

    #[test]
    fn ssh_is_protected_even_if_config_list_is_empty() {
        let config = ServerConfig {
            protected_ports: Vec::new(),
            allow_privileged_ports: true,
            ..ServerConfig::default()
        };
        assert!(config.effective_protected_ports().contains(&22));
    }

    #[test]
    fn rejects_listener_conflicts_and_invalid_ranges() {
        let mut config = ServerConfig {
            web_port: 7710,
            ..ServerConfig::default()
        };
        assert!(config.validate().is_err());

        config.web_port = 7711;
        config.port_min = 9000;
        config.port_max = 8000;
        assert!(config.validate().is_err());
    }

    #[test]
    fn default_configuration_is_valid() {
        ServerConfig::default().validate().unwrap();
    }
}
