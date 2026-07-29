use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
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
}
