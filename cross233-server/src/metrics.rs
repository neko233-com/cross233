use crate::service::SharedServiceState;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const MAX_HISTORY_POINTS: usize = 600;
const METRICS_INTERVAL_SECS: u64 = 1;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetricPoint {
    pub ts: u64,
    pub total_tx: u64,
    pub total_rx: u64,
    pub total_conns: u32,
    pub active_clients: u32,
    pub active_services: u32,
    pub bandwidth_tx: u64,
    pub bandwidth_rx: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceMetricPoint {
    pub ts: u64,
    pub conns: u32,
    pub tx: u64,
    pub rx: u64,
    pub bw_tx: u64,
    pub bw_rx: u64,
}

#[derive(Debug)]
pub struct MetricsCollector {
    history: RwLock<VecDeque<MetricPoint>>,
    service_history: RwLock<Vec<(String, VecDeque<ServiceMetricPoint>)>>,
    last_total_tx: std::sync::atomic::AtomicU64,
    last_total_rx: std::sync::atomic::AtomicU64,
    last_collect: RwLock<Instant>,
}

impl MetricsCollector {
    pub fn new() -> Arc<Self> {
        let m = Arc::new(Self {
            history: RwLock::new(VecDeque::with_capacity(MAX_HISTORY_POINTS)),
            service_history: RwLock::new(Vec::new()),
            last_total_tx: std::sync::atomic::AtomicU64::new(0),
            last_total_rx: std::sync::atomic::AtomicU64::new(0),
            last_collect: RwLock::new(Instant::now()),
        });
        m
    }

    pub async fn collect(&self, service_state: &super::service::SharedServiceState) {
        let services = service_state.get_all_services().await;
        let clients = service_state.get_clients().await;

        let mut total_tx = 0u64;
        let mut total_rx = 0u64;
        let mut total_conns = 0u32;
        let active_services = services.len() as u32;
        let active_clients = clients.len() as u32;

        let prev_tx = self.last_total_tx.swap(0, Ordering::Relaxed);
        let prev_rx = self.last_total_rx.swap(0, Ordering::Relaxed);

        let mut service_data = Vec::new();
        for entry in &services {
            let tx = entry.traffic_tx.load(Ordering::Relaxed);
            let rx = entry.traffic_rx.load(Ordering::Relaxed);
            let conns = entry.current_conns.load(Ordering::Relaxed) as u32;
            total_tx += tx;
            total_rx += rx;
            total_conns += conns;
            service_data.push((entry.config.name.clone(), conns, tx, rx));
        }

        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let elapsed = {
            let mut last = self.last_collect.write().await;
            let el = last.elapsed().as_secs_f64().max(0.1);
            *last = Instant::now();
            el
        };

        let bw_tx = ((total_tx.saturating_sub(prev_tx)) as f64 / elapsed) as u64;
        let bw_rx = ((total_rx.saturating_sub(prev_rx)) as f64 / elapsed) as u64;

        self.last_total_tx.store(total_tx, Ordering::Relaxed);
        self.last_total_rx.store(total_rx, Ordering::Relaxed);

        let point = MetricPoint {
            ts: now_unix,
            total_tx,
            total_rx,
            total_conns,
            active_clients,
            active_services,
            bandwidth_tx: bw_tx,
            bandwidth_rx: bw_rx,
        };

        {
            let mut hist = self.history.write().await;
            hist.push_back(point);
            if hist.len() > MAX_HISTORY_POINTS {
                hist.pop_front();
            }
        }

        {
            let mut svc_hist = self.service_history.write().await;
            for (name, conns, tx, rx) in service_data {
                let found = svc_hist.iter_mut().find(|(n, _)| n == &name);
                let bw = if let Some(prev) = found.as_ref().and_then(|(_, q)| q.back()) {
                    let d_tx = tx.saturating_sub(prev.tx);
                    let d_rx = rx.saturating_sub(prev.rx);
                    (
                        (d_tx as f64 / elapsed) as u64,
                        (d_rx as f64 / elapsed) as u64,
                    )
                } else {
                    (0, 0)
                };
                let svc_point = ServiceMetricPoint {
                    ts: now_unix,
                    conns,
                    tx,
                    rx,
                    bw_tx: bw.0,
                    bw_rx: bw.1,
                };
                match found {
                    Some((_, q)) => {
                        q.push_back(svc_point);
                        if q.len() > MAX_HISTORY_POINTS {
                            q.pop_front();
                        }
                    }
                    None => {
                        let mut q = VecDeque::with_capacity(MAX_HISTORY_POINTS);
                        q.push_back(svc_point);
                        svc_hist.push((name, q));
                    }
                }
            }
            svc_hist.retain(|(name, _)| services.iter().any(|e| e.config.name == *name));
        }
    }

    pub async fn get_history(&self, limit: Option<usize>) -> Vec<MetricPoint> {
        let hist = self.history.read().await;
        let limit = limit.unwrap_or(hist.len()).min(hist.len());
        hist.iter().rev().take(limit).rev().cloned().collect()
    }

    pub async fn get_service_history(
        &self,
        name: &str,
        limit: Option<usize>,
    ) -> Vec<ServiceMetricPoint> {
        let svc_hist = self.service_history.read().await;
        if let Some((_, q)) = svc_hist.iter().find(|(n, _)| n == name) {
            let limit = limit.unwrap_or(q.len()).min(q.len());
            q.iter().rev().take(limit).rev().cloned().collect()
        } else {
            Vec::new()
        }
    }

    pub async fn start_collector(self: Arc<Self>, service_state: Arc<SharedServiceState>) {
        let mut interval = tokio::time::interval(Duration::from_secs(METRICS_INTERVAL_SECS));
        loop {
            interval.tick().await;
            self.collect(&service_state).await;
        }
    }
}
