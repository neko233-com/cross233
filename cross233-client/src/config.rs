use anyhow::{anyhow, Context, Result};
use cross233_protocol::Service;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn default_server() -> String {
    "x.x.x.x:7710".to_string()
}

fn default_server_name() -> String {
    "cross233".to_string()
}

fn default_client_id() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| {
            let id = cross233_protocol::random_id();
            format!("client-{}", &id[..8])
        })
}

fn default_max_tunnels() -> u32 {
    64
}

fn default_qcp_port() -> u16 {
    7713
}

fn default_qcp_tunnel_port() -> u16 {
    7714
}

fn default_transport() -> String {
    "tcp".to_string()
}

fn default_transport_cache_file() -> String {
    ".cross233/transport-cache.json".to_string()
}

fn default_transport_cache_ttl_secs() -> u64 {
    86_400
}

fn default_transport_probe_timeout_ms() -> u64 {
    // QCP's loss-tolerant SYN retry window is five seconds. Leave a small
    // margin for TLS and the authenticated tunnel hello before TCP fallback.
    6_000
}

fn default_web_addr() -> String {
    "127.0.0.1:7721".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    #[serde(default = "default_server")]
    pub server: String,
    #[serde(default = "default_server_name")]
    pub server_name: String,
    #[serde(default)]
    pub auth_key: String,
    #[serde(default)]
    pub auth_key_file: String,
    #[serde(default)]
    pub key_file: String,
    #[serde(default)]
    pub ca_file: String,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default = "default_client_id")]
    pub client_id: String,
    #[serde(default = "default_max_tunnels")]
    pub max_tunnels: u32,
    #[serde(default = "default_qcp_port")]
    pub qcp_port: u16,
    #[serde(default = "default_qcp_tunnel_port")]
    pub qcp_tunnel_port: u16,
    /// Client-to-server transport: tcp, qcp, or auto.
    #[serde(default = "default_transport")]
    pub transport: String,
    /// Local-only cache path for auto transport decisions.
    #[serde(default = "default_transport_cache_file")]
    pub transport_cache_file: String,
    #[serde(default = "default_transport_cache_ttl_secs")]
    pub transport_cache_ttl_secs: u64,
    #[serde(default = "default_transport_probe_timeout_ms")]
    pub transport_probe_timeout_ms: u64,
    #[serde(default = "default_web_addr")]
    pub web_addr: String,
    #[serde(default)]
    pub reload_interval: u32,
    #[serde(default)]
    pub includes: Vec<String>,
    #[serde(default)]
    pub services: Vec<Service>,
    #[serde(default)]
    pub visitors: Vec<VisitorConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VisitorConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub server_name: String,
    pub secret_key: String,
    pub bind_addr: String,
    pub bind_port: u16,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server: default_server(),
            server_name: default_server_name(),
            auth_key: String::new(),
            auth_key_file: String::new(),
            key_file: String::new(),
            ca_file: String::new(),
            insecure: false,
            client_id: default_client_id(),
            max_tunnels: default_max_tunnels(),
            qcp_port: default_qcp_port(),
            qcp_tunnel_port: default_qcp_tunnel_port(),
            transport: default_transport(),
            transport_cache_file: default_transport_cache_file(),
            transport_cache_ttl_secs: default_transport_cache_ttl_secs(),
            transport_probe_timeout_ms: default_transport_probe_timeout_ms(),
            web_addr: default_web_addr(),
            reload_interval: 0,
            includes: Vec::new(),
            services: Vec::new(),
            visitors: Vec::new(),
        }
    }
}

impl ClientConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;

        let mut cfg: Self = match path.extension().and_then(|e| e.to_str()) {
            Some("toml") => toml::from_str(&content)?,
            Some("yaml") | Some("yml") => serde_yaml::from_str(&content)?,
            Some("json") => serde_json::from_str(&content)?,
            _ => {
                if let Ok(c) = toml::from_str(&content) {
                    c
                } else if let Ok(c) = serde_yaml::from_str(&content) {
                    c
                } else {
                    serde_json::from_str(&content)?
                }
            }
        };

        let base = path.parent().unwrap_or_else(|| Path::new("."));
        cfg.load_auth_key(base)?;
        cfg.load_includes(base)?;
        cfg.resolve_transport_cache_path(base)?;

        Ok(cfg)
    }

    fn resolve_transport_cache_path(&mut self, base: &Path) -> Result<()> {
        let base = if base.is_absolute() {
            base.to_path_buf()
        } else {
            std::env::current_dir()?.join(base)
        };
        let configured = PathBuf::from(&self.transport_cache_file);
        let path = if configured.as_os_str().is_empty() {
            base.join(default_transport_cache_file())
        } else if configured.is_absolute() {
            configured
        } else {
            base.join(configured)
        };
        self.transport_cache_file = path.to_string_lossy().into_owned();
        Ok(())
    }

    fn load_includes(&mut self, base: &Path) -> Result<()> {
        let mut service_names: HashMap<String, ()> = HashMap::new();
        for svc in &self.services {
            service_names.insert(svc.name.clone(), ());
        }

        for pattern in &self.includes {
            let pattern_path = if Path::new(pattern).is_absolute() {
                pattern.clone()
            } else {
                base.join(pattern).to_string_lossy().to_string()
            };

            for entry in glob::glob(&pattern_path)? {
                let entry = entry?;
                if entry.is_file() {
                    let content = std::fs::read_to_string(&entry)?;
                    let services: Vec<Service> = match entry.extension().and_then(|e| e.to_str()) {
                        Some("toml") => {
                            let val: toml::Value = toml::from_str(&content)?;
                            if let Some(svcs) = val.get("services").and_then(|v| v.as_array()) {
                                let mut result = Vec::new();
                                for v in svcs {
                                    let s = toml::to_string(v)?;
                                    result.push(toml::from_str(&s)?);
                                }
                                result
                            } else {
                                continue;
                            }
                        }
                        Some("yaml") | Some("yml") => {
                            let val: serde_yaml::Value = serde_yaml::from_str(&content)?;
                            if let Some(svcs) = val.get("services").and_then(|v| v.as_sequence()) {
                                svcs.iter()
                                    .cloned()
                                    .map(serde_yaml::from_value)
                                    .collect::<std::result::Result<Vec<_>, _>>()?
                            } else {
                                continue;
                            }
                        }
                        Some("json") => {
                            serde_json::from_str::<Vec<Service>>(&content).or_else(|_| {
                                let val: serde_json::Value = serde_json::from_str(&content)?;
                                val.get("services")
                                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                                    .ok_or_else(|| anyhow!("no services in {}", entry.display()))
                            })?
                        }
                        _ => continue,
                    };
                    for svc in services {
                        if !service_names.contains_key(&svc.name) {
                            service_names.insert(svc.name.clone(), ());
                            self.services.push(svc);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn load_auth_key(&mut self, base: &Path) -> Result<()> {
        if !self.auth_key.is_empty() {
            return Ok(());
        }

        let (configured_path, legacy_key_file) = if !self.auth_key_file.is_empty() {
            (&self.auth_key_file, false)
        } else if !self.key_file.is_empty() {
            (&self.key_file, true)
        } else {
            return Ok(());
        };

        let path = if Path::new(configured_path).is_absolute() {
            Path::new(configured_path).to_path_buf()
        } else {
            base.join(configured_path)
        };
        let key = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read auth key file {}", path.display()))?;
        let key = key.trim();

        // Older clients used key_file for their HMAC key. Preserve that
        // configuration, but do not mistake a PEM client certificate for an
        // authentication key.
        if legacy_key_file && key.starts_with("-----BEGIN") {
            return Ok(());
        }
        if key.is_empty() {
            anyhow::bail!("auth key file {} is empty", path.display());
        }

        self.auth_key = key.to_string();
        if legacy_key_file {
            self.key_file.clear();
        }
        Ok(())
    }

    pub fn parse_services_cli(spec: &str) -> Result<Vec<Service>> {
        let mut services = Vec::new();
        for item in spec.split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            let parts: Vec<&str> = item.splitn(5, ':').collect();
            if parts.len() < 4 {
                return Err(anyhow!(
                    "invalid service spec '{}': expected name:type:localHost:localPort[:remotePort]",
                    item
                ));
            }
            let name = parts[0].to_string();
            let ty = parts[1].to_string();
            let local_addr = format!("{}:{}", parts[2], parts[3]);
            let remote_port = parts.get(4).and_then(|p| p.parse().ok()).unwrap_or(0);
            services.push(Service {
                name,
                ty: Some(ty),
                local_addr,
                remote_port,
                ..Default::default()
            });
        }
        Ok(services)
    }

    pub fn fingerprint<P: AsRef<Path>>(path: P) -> Result<u64> {
        let path = path.as_ref();
        let meta = std::fs::metadata(path)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        meta.len().hash(&mut hasher);
        meta.modified()
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            })
            .unwrap_or(0)
            .hash(&mut hasher);
        if let Some(parent) = path.parent() {
            for pattern in &["*.toml", "*.yaml", "*.yml", "*.json"] {
                let pat = parent.join(pattern).to_string_lossy().to_string();
                if let Ok(entries) = glob::glob(&pat) {
                    for entry in entries.flatten() {
                        if let Ok(m) = std::fs::metadata(&entry) {
                            m.len().hash(&mut hasher);
                        }
                    }
                }
            }
        }
        Ok(hasher.finish())
    }

    pub fn enabled_services(&self) -> Vec<Service> {
        self.services
            .iter()
            .filter(|s| s.is_enabled())
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::ClientConfig;

    #[test]
    fn parses_legacy_snake_case_service_keys() {
        let cfg: ClientConfig = toml::from_str(
            r#"
server = "127.0.0.1:7710"
auth_key = "test"

[[services]]
name = "legacy"
type = "tcp"
local_addr = "127.0.0.1:3000"
remote_port = 60080
"#,
        )
        .unwrap();

        assert_eq!(cfg.services.len(), 1);
        assert_eq!(cfg.services[0].local_addr, "127.0.0.1:3000");
        assert_eq!(cfg.services[0].remote_port, 60080);
    }

    #[test]
    fn loads_legacy_key_file_as_auth_key() {
        let root = std::env::temp_dir().join(format!(
            "cross233-client-config-test-{}",
            cross233_protocol::random_id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("cross233-auth.key"), "legacy-auth-key\n").unwrap();
        std::fs::write(
            root.join("client.toml"),
            r#"
server = "127.0.0.1:7710"
key_file = "cross233-auth.key"

[[services]]
name = "legacy"
type = "tcp"
local_addr = "127.0.0.1:3000"
remote_port = 60080
"#,
        )
        .unwrap();

        let cfg = ClientConfig::load(root.join("client.toml")).unwrap();
        assert_eq!(cfg.auth_key, "legacy-auth-key");
        assert!(cfg.key_file.is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_transport_cache_below_config_directory() {
        let root = std::env::temp_dir().join(format!(
            "cross233-client-cache-path-test-{}",
            cross233_protocol::random_id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("client.toml");
        std::fs::write(
            &config_path,
            r#"
server = "127.0.0.1:7710"
auth_key = "test"

[[services]]
name = "test"
type = "tcp"
localAddr = "127.0.0.1:1"
remotePort = 60080
"#,
        )
        .unwrap();

        let cfg = ClientConfig::load(&config_path).unwrap();
        assert_eq!(
            std::path::PathBuf::from(cfg.transport_cache_file),
            root.join(".cross233").join("transport-cache.json")
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
