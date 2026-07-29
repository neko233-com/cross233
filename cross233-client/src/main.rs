use clap::Parser;
use cross233_client::{Client, ClientConfig};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "cross233-client", version, about = "Cross233 tunnel client")]
struct Args {
    #[arg(short = 'c', long, help = "Config file path (toml/yaml/json)")]
    config: Option<PathBuf>,

    #[arg(long, help = "Server address, e.g. x.x.x.x:7710")]
    server: Option<String>,

    #[arg(long, help = "Auth key")]
    auth_key: Option<String>,

    #[arg(long, help = "Client ID")]
    client_id: Option<String>,

    #[arg(
        short = 's',
        long,
        help = "Quick services: name:type:host:port[:remotePort],..."
    )]
    services: Option<String>,

    #[arg(
        long,
        default_value_t = false,
        help = "Skip TLS certificate verification"
    )]
    insecure: bool,

    #[arg(long, help = "Local web admin address")]
    web_addr: Option<String>,

    #[arg(long, help = "CA certificate file")]
    ca_file: Option<String>,

    #[arg(long, help = "Client certificate key file")]
    key_file: Option<String>,

    #[arg(long, help = "Server hostname for TLS verification")]
    server_name: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cross233_client=debug")
            }),
        )
        .with_target(false)
        .init();

    let args = Args::parse();

    let mut cfg = if let Some(path) = &args.config {
        ClientConfig::load(path)?
    } else {
        ClientConfig::default()
    };

    if let Some(server) = args.server {
        cfg.server = server;
    }
    if let Some(key) = args.auth_key {
        cfg.auth_key = key;
    }
    if let Some(cid) = args.client_id {
        cfg.client_id = cid;
    }
    if args.insecure {
        cfg.insecure = true;
    }
    if let Some(wa) = args.web_addr {
        cfg.web_addr = wa;
    }
    if let Some(ca) = args.ca_file {
        cfg.ca_file = ca;
    }
    if let Some(kf) = args.key_file {
        cfg.key_file = kf;
    }
    if let Some(sn) = args.server_name {
        cfg.server_name = sn;
    }
    if let Some(spec) = args.services {
        let services = ClientConfig::parse_services_cli(&spec)?;
        cfg.services.extend(services);
    }

    if cfg.auth_key.is_empty() {
        tracing::warn!("no auth_key configured; authentication may fail");
    }
    if cfg.services.is_empty() && cfg.visitors.is_empty() {
        anyhow::bail!("no services or visitors configured; use --services or a config file");
    }

    let state_data = cross233_client::WebStateData {
        server_addr: cfg.server.clone(),
        client_id: cfg.client_id.clone(),
        services: cfg.enabled_services(),
        ..Default::default()
    };
    let web_state = std::sync::Arc::new(tokio::sync::RwLock::new(state_data));

    let shutdown = std::sync::Arc::new(tokio::sync::Notify::new());
    let client = Client::new(cfg.clone(), web_state.clone(), shutdown.clone())?;

    let web_addr = cfg.web_addr.clone();
    let ws = web_state.clone();
    let web_shutdown = shutdown.clone();
    let web_task = tokio::spawn(async move {
        tokio::select! {
            _ = web_shutdown.notified() => {}
            r = cross233_client::web::start_web_server(&web_addr, ws) => {
                if let Err(e) = r {
                    tracing::error!("web server error: {e}");
                }
            }
        }
    });

    if cfg.reload_interval > 0 {
        if let Some(config_path) = args.config.clone() {
            let (reload_tx, _) = tokio::sync::watch::channel(None);
            let config_path = config_path.clone();
            tokio::spawn(async move {
                let _ = cross233_client::config_reload::watch_config(
                    config_path,
                    cfg.reload_interval,
                    reload_tx,
                )
                .await;
            });
        }
    }

    let client_task = tokio::spawn(async move {
        if let Err(e) = client.run().await {
            tracing::error!("client error: {e}");
        }
    });

    let _ = tokio::signal::ctrl_c().await;
    shutdown.notify_waiters();
    let _ = web_task.await;
    let _ = client_task.await;

    tracing::info!("client stopped");
    Ok(())
}
