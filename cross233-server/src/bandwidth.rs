use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct BandwidthLimiter {
    inner: Arc<Mutex<BandwidthInner>>,
}

#[derive(Debug)]
struct BandwidthInner {
    bytes_per_second: u64,
    tokens: f64,
    last_refill: Instant,
    capacity: f64,
}

impl BandwidthLimiter {
    pub fn new(bytes_per_second: u64) -> Self {
        let capacity = bytes_per_second.max(1) as f64;
        Self {
            inner: Arc::new(Mutex::new(BandwidthInner {
                bytes_per_second,
                tokens: capacity,
                last_refill: Instant::now(),
                capacity,
            })),
        }
    }

    pub fn new_kbps(kbps: u64) -> Self {
        Self::new(kbps * 1024 / 8)
    }

    pub async fn acquire(&self, n: u64) {
        if n == 0 {
            return;
        }
        loop {
            let wait = {
                let mut inner = self.inner.lock().await;
                let now = Instant::now();
                let elapsed = now.duration_since(inner.last_refill).as_secs_f64();
                inner.tokens =
                    (inner.tokens + elapsed * inner.bytes_per_second as f64).min(inner.capacity);
                inner.last_refill = now;

                if inner.tokens >= n as f64 {
                    inner.tokens -= n as f64;
                    None
                } else {
                    let deficit = n as f64 - inner.tokens;
                    let wait_secs = deficit / inner.bytes_per_second as f64;
                    Some(Duration::from_secs_f64(wait_secs.max(0.001)))
                }
            };

            match wait {
                None => break,
                Some(d) => tokio::time::sleep(d).await,
            }
        }
    }
}
