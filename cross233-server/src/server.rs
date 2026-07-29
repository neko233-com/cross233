use crate::auth::AuthState;
use crate::config::ServerConfig;
use crate::control::run_control_listener;
use crate::crypto::setup_tls;
use crate::health_check::run_health_monitor;
use crate::http_vhost::run_http_vhost;
use crate::https_vhost::run_https_vhost;
use crate::metrics::MetricsCollector;
use crate::qcp::{run_qcp_listener, run_qcp_tunnel_listener};
use crate::service::SharedServiceState;
use crate::tcpmux::run_tcpmux;
use crate::udp::UdpManager;
use crate::web::run_web_server;
use anyhow::Context;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

pub struct Server {
    config: ServerConfig,
    tls_acceptor: TlsAcceptor,
    fingerprint: String,
    auth_key: String,
}

impl Server {
    pub async fn new(mut config: ServerConfig) -> anyhow::Result<Self> {
        let auth_key =
            crate::crypto::load_or_create_auth_key(&config.auth_key_file, &config.auth_key)?;
        config.auth_key = auth_key.clone();

        let tls_setup = setup_tls(&config.cert_file, &config.key_file)?;
        let acceptor = TlsAcceptor::from(Arc::new(tls_setup.server_config));

        Ok(Self {
            config,
            tls_acceptor: acceptor,
            fingerprint: tls_setup.fingerprint,
            auth_key,
        })
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn auth_key(&self) -> &str {
        &self.auth_key
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let state = SharedServiceState::new(
            self.config.port_min,
            self.config.port_max,
            self.config.qcp_port,
        );

        let metrics = MetricsCollector::new();

        let metrics_collector = metrics.clone();
        let metrics_state = state.clone();
        tokio::spawn(async move {
            metrics_collector.start_collector(metrics_state).await;
        });

        let auth = AuthState::new(self.auth_key.clone());

        let udp_mgr = UdpManager::new(state.clone(), self.config.bind.clone());

        let control_addr = format!("{}:{}", self.config.bind, self.config.control_port);
        let control_listener = TcpListener::bind(&control_addr)
            .await
            .with_context(|| format!("bind control {}", control_addr))?;
        tracing::info!(addr = %control_addr, "control listener started");

        let mut handles = Vec::new();

        if self.config.http_vhost_port != 0 {
            let http_vhost_addr = format!("{}:{}", self.config.bind, self.config.http_vhost_port);
            let http_state = state.clone();
            handles.push(tokio::spawn(async move {
                if let Err(e) = run_http_vhost(http_state, &http_vhost_addr).await {
                    tracing::error!("http vhost error: {}", e);
                }
            }));
        }

        if self.config.https_vhost_port != 0 {
            let https_vhost_addr = format!("{}:{}", self.config.bind, self.config.https_vhost_port);
            let https_state = state.clone();
            handles.push(tokio::spawn(async move {
                if let Err(e) = run_https_vhost(https_state, &https_vhost_addr).await {
                    tracing::error!("https vhost error: {}", e);
                }
            }));
        }

        if self.config.tcpmux_port != 0 {
            let tcpmux_addr = format!("{}:{}", self.config.bind, self.config.tcpmux_port);
            let tcpmux_state = state.clone();
            handles.push(tokio::spawn(async move {
                if let Err(e) = run_tcpmux(tcpmux_state, &tcpmux_addr).await {
                    tracing::error!("tcpmux error: {}", e);
                }
            }));
        }

        if self.config.qcp_port != 0 {
            let qcp_addr = format!("{}:{}", self.config.bind, self.config.qcp_port);
            let qcp_state = state.clone();
            handles.push(tokio::spawn(async move {
                if let Err(e) = run_qcp_listener(qcp_state, &qcp_addr).await {
                    tracing::error!("qcp error: {}", e);
                }
            }));
        }

        if self.config.qcp_tunnel_port != 0 {
            let qcp_tunnel_addr = format!("{}:{}", self.config.bind, self.config.qcp_tunnel_port);
            let qcp_tunnel_state = state.clone();
            let qcp_tunnel_acceptor = self.tls_acceptor.clone();
            let qcp_tunnel_auth = auth.clone();
            let qcp_timeout = self.config.handshake_timeout;
            handles.push(tokio::spawn(async move {
                if let Err(e) = run_qcp_tunnel_listener(
                    qcp_tunnel_state,
                    &qcp_tunnel_addr,
                    qcp_tunnel_acceptor,
                    qcp_tunnel_auth,
                    qcp_timeout,
                )
                .await
                {
                    tracing::error!("QCP tunnel listener error: {}", e);
                }
            }));
        }

        if self.config.web_port != 0 {
            let web_addr = format!("{}:{}", self.config.bind, self.config.web_port);
            let web_state = state.clone();
            let web_config = self.config.clone();
            let web_metrics = metrics.clone();
            handles.push(tokio::spawn(async move {
                if let Err(e) = run_web_server(web_state, web_config, &web_addr, web_metrics).await
                {
                    tracing::error!("web server error: {}", e);
                }
            }));
        }

        let health_state = state.clone();
        handles.push(tokio::spawn(async move {
            run_health_monitor(health_state).await;
        }));

        let control_acceptor = self.tls_acceptor.clone();
        let control_auth = auth.clone();
        let control_state = state.clone();
        let control_udp = Some(udp_mgr);
        let control_qcp_port = self.config.qcp_port;
        let control_qcp_tunnel_port = self.config.qcp_tunnel_port;
        let control_handshake_timeout =
            std::time::Duration::from_secs(self.config.handshake_timeout.max(1) as u64);

        let control_handle = tokio::spawn(async move {
            run_control_listener(
                control_listener,
                control_acceptor,
                control_auth,
                control_state,
                control_udp,
                control_qcp_port,
                control_qcp_tunnel_port,
                control_handshake_timeout,
            )
            .await;
        });

        tracing::info!("cross233-server started");
        tracing::info!("  control port: {}", self.config.control_port);
        if self.config.web_port != 0 {
            tracing::info!("  web port:     {}", self.config.web_port);
        }
        if self.config.http_vhost_port != 0 {
            tracing::info!("  http vhost:   {}", self.config.http_vhost_port);
        }
        if self.config.https_vhost_port != 0 {
            tracing::info!("  https vhost:  {}", self.config.https_vhost_port);
        }
        if self.config.tcpmux_port != 0 {
            tracing::info!("  tcpmux port:  {}", self.config.tcpmux_port);
        }
        if self.config.qcp_port != 0 {
            tracing::info!("  qcp port:     {}", self.config.qcp_port);
        }
        if self.config.qcp_tunnel_port != 0 {
            tracing::info!("  qcp tunnel:   {}", self.config.qcp_tunnel_port);
        }
        tracing::info!("  fingerprint:  {}", self.fingerprint);

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutdown signal received");
            }
            _ = control_handle => {
                tracing::error!("control listener ended unexpectedly");
            }
        }

        for h in handles {
            h.abort();
        }

        Ok(())
    }
}
