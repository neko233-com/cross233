use clap::Parser;
use cross233_server::config::ServerConfig;
use cross233_server::server::Server;

#[derive(Parser, Debug)]
#[command(name = "cross233-server", version, about = "Cross233 tunnel server")]
struct Args {
    #[arg(short = 'c', long, help = "Config file path")]
    config: Option<String>,

    #[arg(long, help = "Bind address")]
    bind: Option<String>,

    #[arg(long, help = "Control port")]
    port: Option<u16>,

    #[arg(long, help = "Web management port")]
    web_port: Option<u16>,

    #[arg(long, help = "Auth key")]
    auth_key: Option<String>,

    #[arg(long, help = "TLS cert file")]
    cert_file: Option<String>,

    #[arg(long, help = "TLS key file")]
    key_file: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let config_path = args.config.clone();
    let mut config = ServerConfig::load(config_path.as_deref()).unwrap_or_default();

    if let Some(v) = args.bind {
        config.bind = v;
    }
    if let Some(v) = args.port {
        config.control_port = v;
    }
    if let Some(v) = args.web_port {
        config.web_port = v;
    }
    if let Some(v) = args.auth_key {
        config.auth_key = v;
    }
    if let Some(v) = args.cert_file {
        config.cert_file = v;
    }
    if let Some(v) = args.key_file {
        config.key_file = v;
    }

    let server = Server::new(config).await?;

    println!("cross233-server");
    println!("  fingerprint: {}", server.fingerprint());

    server.run().await?;
    Ok(())
}
