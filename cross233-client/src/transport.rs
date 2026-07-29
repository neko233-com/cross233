use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CACHE_VERSION: u32 = 1;
const MAX_ERROR_CHARS: usize = 240;

/// A transport used between the cross233 client and server.
///
/// This is deliberately separate from a service's `type` (`tcp`, `static`,
/// `qcp`, ...). It describes the authenticated control and tunnel channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    Tcp,
    Qcp,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Qcp => "qcp",
        }
    }

    pub fn alternate(self) -> Self {
        match self {
            Self::Tcp => Self::Qcp,
            Self::Qcp => Self::Tcp,
        }
    }
}

impl std::fmt::Display for TransportKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    Auto,
    Tcp,
    Qcp,
}

impl TransportMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "tcp" | "" => Ok(Self::Tcp),
            "qcp" => Ok(Self::Qcp),
            other => Err(anyhow!(
                "invalid transport '{}': expected auto, tcp, or qcp",
                other
            )),
        }
    }

    pub fn is_auto(self) -> bool {
        matches!(self, Self::Auto)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceSource {
    Forced,
    Cache,
    Probe,
    Fallback,
}

impl ChoiceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forced => "forced",
            Self::Cache => "cache",
            Self::Probe => "probe",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportChoice {
    pub kind: TransportKind,
    pub source: ChoiceSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: BTreeMap<String, CacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheEntry {
    selected: String,
    updated_at_unix: u64,
    last_success_at_unix: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    qcp_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tcp_error: Option<String>,
}

/// Local-only cache for the result of a complete authenticated connection.
/// The key contains endpoints and ports only; authentication keys and local
/// service paths are intentionally never stored here.
#[derive(Debug, Clone)]
pub struct TransportCache {
    path: PathBuf,
    ttl: Duration,
}

impl TransportCache {
    pub fn new(path: impl Into<PathBuf>, ttl_secs: u64) -> Self {
        Self {
            path: path.into(),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn key(server: &str, server_name: &str, qcp_tunnel_port: u16) -> String {
        format!(
            "v1|server={}|server_name={}|qcp_tunnel_port={}",
            server, server_name, qcp_tunnel_port
        )
    }

    pub fn choices(&self, mode: TransportMode, key: &str) -> Vec<TransportChoice> {
        match mode {
            TransportMode::Tcp => vec![TransportChoice {
                kind: TransportKind::Tcp,
                source: ChoiceSource::Forced,
            }],
            TransportMode::Qcp => vec![TransportChoice {
                kind: TransportKind::Qcp,
                source: ChoiceSource::Forced,
            }],
            TransportMode::Auto => {
                if let Some(kind) = self.preferred(key) {
                    vec![
                        TransportChoice {
                            kind,
                            source: ChoiceSource::Cache,
                        },
                        TransportChoice {
                            kind: kind.alternate(),
                            source: ChoiceSource::Fallback,
                        },
                    ]
                } else {
                    vec![
                        TransportChoice {
                            kind: TransportKind::Qcp,
                            source: ChoiceSource::Probe,
                        },
                        TransportChoice {
                            kind: TransportKind::Tcp,
                            source: ChoiceSource::Fallback,
                        },
                    ]
                }
            }
        }
    }

    pub fn preferred(&self, key: &str) -> Option<TransportKind> {
        if self.ttl.is_zero() {
            return None;
        }
        let cache = self.read().ok()?;
        if cache.version != CACHE_VERSION {
            return None;
        }
        let entry = cache.entries.get(key)?;
        let now = now_unix();
        if now.saturating_sub(entry.updated_at_unix) > self.ttl.as_secs() {
            return None;
        }
        parse_kind(&entry.selected)
    }

    pub fn note_success(&self, key: &str, kind: TransportKind) -> Result<()> {
        let mut cache = self.read().unwrap_or_default();
        cache.version = CACHE_VERSION;
        let now = now_unix();
        let entry = cache.entries.entry(key.to_string()).or_default();
        entry.selected = kind.as_str().to_string();
        entry.updated_at_unix = now;
        entry.last_success_at_unix = now;
        match kind {
            TransportKind::Qcp => entry.qcp_error = None,
            TransportKind::Tcp => entry.tcp_error = None,
        }
        self.write(&cache)
    }

    pub fn note_failure(
        &self,
        key: &str,
        kind: TransportKind,
        error: &anyhow::Error,
    ) -> Result<()> {
        let mut cache = self.read().unwrap_or_default();
        cache.version = CACHE_VERSION;
        let entry = cache.entries.entry(key.to_string()).or_default();
        let text = compact_error(error);
        match kind {
            TransportKind::Qcp => entry.qcp_error = Some(text),
            TransportKind::Tcp => entry.tcp_error = Some(text),
        }
        self.write(&cache)
    }

    fn read(&self) -> Result<CacheFile> {
        let data = match fs::read(&self.path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(CacheFile::default()),
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("read transport cache {}", self.path.display()))
            }
        };
        serde_json::from_slice(&data)
            .with_context(|| format!("parse transport cache {}", self.path.display()))
    }

    fn write(&self, cache: &CacheFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("create transport cache directory {}", parent.display())
            })?;
        }

        let data = serde_json::to_vec_pretty(cache)?;
        let extension = format!(
            "{}.{}.tmp",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let tmp = self.path.with_extension(extension);
        let mut file = File::options()
            .create_new(true)
            .write(true)
            .open(&tmp)
            .with_context(|| format!("create transport cache temp {}", tmp.display()))?;
        file.write_all(&data)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);

        if let Err(rename_error) = fs::rename(&tmp, &self.path) {
            // Windows cannot replace an existing file with rename. The cache is
            // diagnostic state, so a short replace window is acceptable.
            if self.path.exists() {
                fs::remove_file(&self.path)
                    .with_context(|| format!("replace transport cache {}", self.path.display()))?;
                fs::rename(&tmp, &self.path)
                    .with_context(|| format!("move transport cache {}", self.path.display()))?;
            } else {
                let _ = fs::remove_file(&tmp);
                return Err(rename_error)
                    .with_context(|| format!("replace transport cache {}", self.path.display()));
            }
        }
        Ok(())
    }
}

fn parse_kind(value: &str) -> Option<TransportKind> {
    match value {
        "tcp" => Some(TransportKind::Tcp),
        "qcp" => Some(TransportKind::Qcp),
        _ => None,
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn compact_error(error: &anyhow::Error) -> String {
    format!("{error:#}").chars().take(MAX_ERROR_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_prefers_qcp_then_tcp_without_cache() {
        let cache =
            TransportCache::new(std::env::temp_dir().join("cross233-no-such-cache.json"), 60);
        let choices = cache.choices(TransportMode::Auto, "test");
        assert_eq!(choices[0].kind, TransportKind::Qcp);
        assert_eq!(choices[0].source, ChoiceSource::Probe);
        assert_eq!(choices[1].kind, TransportKind::Tcp);
    }

    #[test]
    fn cached_success_is_reused_and_does_not_include_credentials() {
        let root = std::env::temp_dir().join(format!(
            "cross233-transport-cache-test-{}",
            cross233_protocol::random_id()
        ));
        let path = root.join(".cross233").join("transport-cache.json");
        let cache = TransportCache::new(&path, 60);
        let key = TransportCache::key("127.0.0.1:7710", "cross233", 7714);
        cache.note_success(&key, TransportKind::Qcp).unwrap();

        let choices = cache.choices(TransportMode::Auto, &key);
        assert_eq!(choices[0].kind, TransportKind::Qcp);
        assert_eq!(choices[0].source, ChoiceSource::Cache);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("auth_key"));
        assert!(!text.contains("secret"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn expired_cache_reprobes_qcp() {
        let root = std::env::temp_dir().join(format!(
            "cross233-transport-cache-expired-test-{}",
            cross233_protocol::random_id()
        ));
        let path = root.join("transport-cache.json");
        let cache = TransportCache::new(&path, 0);
        let key = "key";
        cache.note_success(key, TransportKind::Tcp).unwrap();

        let choices = cache.choices(TransportMode::Auto, key);
        assert_eq!(choices[0].kind, TransportKind::Qcp);
        assert_eq!(choices[0].source, ChoiceSource::Probe);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_transport_modes() {
        assert_eq!(TransportMode::parse("auto").unwrap(), TransportMode::Auto);
        assert_eq!(TransportMode::parse("TCP").unwrap(), TransportMode::Tcp);
        assert!(TransportMode::parse("udp").is_err());
    }
}
