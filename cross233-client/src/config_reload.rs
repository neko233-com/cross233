use crate::config::ClientConfig;
use anyhow::Result;
use std::path::PathBuf;
use tokio::sync::watch;
use tokio::time::{interval, Duration};

pub async fn watch_config(
    config_path: PathBuf,
    interval_secs: u32,
    reload_tx: watch::Sender<Option<ClientConfig>>,
) -> Result<()> {
    if interval_secs == 0 {
        return Ok(());
    }

    let mut last_fp = ClientConfig::fingerprint(&config_path).unwrap_or(0);
    let mut tick = interval(Duration::from_secs(interval_secs as u64));
    tick.tick().await;

    loop {
        tick.tick().await;
        match ClientConfig::fingerprint(&config_path) {
            Ok(fp) if fp != last_fp => match ClientConfig::load(&config_path) {
                Ok(new_cfg) => {
                    tracing::info!("config changed, reloading");
                    last_fp = fp;
                    let _ = reload_tx.send(Some(new_cfg));
                }
                Err(e) => {
                    tracing::warn!("config reload error: {e}");
                }
            },
            _ => {}
        }
    }
}
