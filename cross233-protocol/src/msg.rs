use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Message {
    #[serde(rename = "type", default, skip_serializing_if = "String::is_empty")]
    pub ty: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(
        default,
        rename = "localAddress",
        skip_serializing_if = "Option::is_none"
    )]
    pub local_address: Option<String>,
    #[serde(
        default,
        rename = "remotePort",
        skip_serializing_if = "Option::is_none"
    )]
    pub remote_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub healthy: Option<bool>,
    #[serde(default, rename = "qcpPort", skip_serializing_if = "Option::is_none")]
    pub qcp_port: Option<u16>,
    #[serde(
        default,
        rename = "qcpTunnelPort",
        skip_serializing_if = "Option::is_none"
    )]
    pub qcp_tunnel_port: Option<u16>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<Service>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<Service>,
    #[serde(
        default,
        with = "base64_bytes",
        skip_serializing_if = "Option::is_none"
    )]
    pub data: Option<Vec<u8>>,
}

mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match bytes {
            Some(b) => serializer.serialize_str(&STANDARD.encode(b)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: Option<String> = Option::deserialize(deserializer)?;
        match s {
            Some(s) => STANDARD
                .decode(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

impl Message {
    pub fn new_client_hello(client_id: &str, services: Vec<Service>) -> Self {
        Self {
            ty: "client".to_string(),
            client_id: Some(client_id.to_string()),
            services,
            ..Default::default()
        }
    }

    pub fn new_tunnel_hello(id: &str) -> Self {
        Self {
            ty: "tunnel".to_string(),
            id: Some(id.to_string()),
            ..Default::default()
        }
    }

    pub fn new_visitor_hello(client_id: &str, service_name: &str, id: Option<&str>) -> Self {
        Self {
            ty: "visitor".to_string(),
            client_id: Some(client_id.to_string()),
            service_name: Some(service_name.to_string()),
            id: id.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    pub fn new_challenge(nonce: &str) -> Self {
        Self {
            ty: "challenge".to_string(),
            nonce: Some(nonce.to_string()),
            ..Default::default()
        }
    }

    pub fn new_auth(proof: &str) -> Self {
        Self {
            ty: "auth".to_string(),
            proof: Some(proof.to_string()),
            ..Default::default()
        }
    }

    pub fn new_ready(services: Vec<Service>, qcp_port: u16, qcp_tunnel_port: u16) -> Self {
        Self {
            ty: "ready".to_string(),
            services,
            qcp_port: Some(qcp_port),
            qcp_tunnel_port: Some(qcp_tunnel_port),
            ..Default::default()
        }
    }

    pub fn new_error(msg: &str) -> Self {
        Self {
            ty: "error".to_string(),
            error: Some(msg.to_string()),
            ..Default::default()
        }
    }

    pub fn new_open(id: &str, address: &str, local_address: &str, service: Service) -> Self {
        Self {
            ty: "open".to_string(),
            id: Some(id.to_string()),
            address: Some(address.to_string()),
            local_address: Some(local_address.to_string()),
            service: Some(service),
            ..Default::default()
        }
    }

    pub fn new_reject(id: &str, err: &str) -> Self {
        Self {
            ty: "reject".to_string(),
            id: Some(id.to_string()),
            error: Some(err.to_string()),
            ..Default::default()
        }
    }

    pub fn new_ping() -> Self {
        Self {
            ty: "ping".to_string(),
            ..Default::default()
        }
    }

    pub fn new_pong() -> Self {
        Self {
            ty: "pong".to_string(),
            ..Default::default()
        }
    }

    pub fn new_udp(remote_port: u16, address: &str, data: Vec<u8>, service: Service) -> Self {
        Self {
            ty: "udp".to_string(),
            remote_port: Some(remote_port),
            address: Some(address.to_string()),
            data: Some(data),
            service: Some(service),
            ..Default::default()
        }
    }

    pub fn new_udp_response(remote_port: u16, address: &str, data: Vec<u8>) -> Self {
        Self {
            ty: "udp_response".to_string(),
            remote_port: Some(remote_port),
            address: Some(address.to_string()),
            data: Some(data),
            ..Default::default()
        }
    }

    pub fn new_visitor_udp(id: &str, data: Vec<u8>) -> Self {
        Self {
            ty: "visitor_udp".to_string(),
            id: Some(id.to_string()),
            data: Some(data),
            ..Default::default()
        }
    }

    pub fn new_visitor_udp_response(id: &str, data: Vec<u8>) -> Self {
        Self {
            ty: "visitor_udp_response".to_string(),
            id: Some(id.to_string()),
            data: Some(data),
            ..Default::default()
        }
    }

    pub fn new_health(service_name: &str, healthy: bool) -> Self {
        Self {
            ty: "health".to_string(),
            service_name: Some(service_name.to_string()),
            healthy: Some(healthy),
            ..Default::default()
        }
    }

    pub fn new_close(reason: &str) -> Self {
        Self {
            ty: "close".to_string(),
            error: Some(reason.to_string()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Service {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub ty: Option<String>,

    #[serde(rename = "localAddr", alias = "local_addr")]
    pub local_addr: String,
    #[serde(
        rename = "remotePort",
        alias = "remote_port",
        skip_serializing_if = "is_zero_u16"
    )]
    pub remote_port: u16,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
    #[serde(rename = "routeByHTTPUser", skip_serializing_if = "Option::is_none")]
    pub route_by_http_user: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(rename = "groupKey", skip_serializing_if = "Option::is_none")]
    pub group_key: Option<String>,

    #[serde(rename = "httpUser", skip_serializing_if = "Option::is_none")]
    pub http_user: Option<String>,
    #[serde(rename = "httpPassword", skip_serializing_if = "Option::is_none")]
    pub http_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,

    #[serde(rename = "bandwidthLimitKbps", skip_serializing_if = "is_zero")]
    pub bandwidth_limit_kbps: u64,
    #[serde(rename = "maxConnections", skip_serializing_if = "is_zero_u32")]
    pub max_connections: u32,

    #[serde(skip_serializing_if = "is_false")]
    pub compression: bool,
    #[serde(rename = "proxyProtocol", skip_serializing_if = "is_false")]
    pub proxy_protocol: bool,
    #[serde(
        rename = "proxyProtocolVersion",
        skip_serializing_if = "Option::is_none"
    )]
    pub proxy_protocol_version: Option<String>,

    #[serde(rename = "allowCIDRs", skip_serializing_if = "Vec::is_empty")]
    pub allow_cidrs: Vec<String>,
    #[serde(rename = "denyCIDRs", skip_serializing_if = "Vec::is_empty")]
    pub deny_cidrs: Vec<String>,

    #[serde(rename = "hostHeaderRewrite", skip_serializing_if = "Option::is_none")]
    pub host_header_rewrite: Option<String>,
    #[serde(rename = "requestHeaders", skip_serializing_if = "HashMap::is_empty")]
    pub request_headers: HashMap<String, String>,
    #[serde(rename = "responseHeaders", skip_serializing_if = "HashMap::is_empty")]
    pub response_headers: HashMap<String, String>,

    #[serde(rename = "healthCheck", skip_serializing_if = "Option::is_none")]
    pub health_check: Option<String>,
    #[serde(rename = "healthInterval", skip_serializing_if = "is_zero_u32")]
    pub health_interval: u32,
    #[serde(rename = "healthTimeout", skip_serializing_if = "is_zero_u32")]
    pub health_timeout: u32,
    #[serde(rename = "healthMaxFailed", skip_serializing_if = "is_zero_u32")]
    pub health_max_failed: u32,
    #[serde(rename = "healthPath", skip_serializing_if = "Option::is_none")]
    pub health_path: Option<String>,
    #[serde(rename = "healthHeaders", skip_serializing_if = "HashMap::is_empty")]
    pub health_headers: HashMap<String, String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub annotations: HashMap<String, String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}
fn is_zero_u16(v: &u16) -> bool {
    *v == 0
}
fn is_false(v: &bool) -> bool {
    !*v
}

impl Service {
    pub fn effective_type(&self) -> &str {
        self.ty.as_deref().unwrap_or("tcp")
    }

    pub fn is_vhost(&self) -> bool {
        matches!(self.effective_type(), "http" | "https" | "tcpmux")
    }

    pub fn is_private(&self) -> bool {
        matches!(self.effective_type(), "stcp" | "sudp")
    }

    pub fn is_udp(&self) -> bool {
        matches!(self.effective_type(), "udp" | "sudp")
    }

    pub fn is_tcp(&self) -> bool {
        let t = self.effective_type();
        matches!(
            t,
            "" | "tcp" | "static" | "http" | "https" | "tcpmux" | "stcp" | "qcp"
        )
    }

    pub fn is_qcp(&self) -> bool {
        self.effective_type() == "qcp"
    }

    pub fn uses_auto_port(&self) -> bool {
        self.remote_port == 0
            && matches!(self.effective_type(), "" | "tcp" | "static" | "udp" | "qcp")
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

pub fn random_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..16).map(|_| rng.gen::<u8>()).collect();
    hex::encode(&bytes)
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..n).map(|_| rng.gen::<u8>()).collect()
}
