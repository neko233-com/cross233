use cross233_protocol::crypto::{compute_proof, generate_nonce, verify_proof};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct PendingAuth {
    nonce: String,
    created_at: Instant,
}

#[derive(Debug, Clone)]
pub struct AuthState {
    auth_key: String,
    pending: Arc<RwLock<HashMap<String, PendingAuth>>>,
}

impl AuthState {
    pub fn new(auth_key: String) -> Self {
        Self {
            auth_key,
            pending: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create_challenge(&self, conn_id: &str) -> String {
        let nonce = generate_nonce();
        {
            let mut pending = self.pending.write().await;
            pending.insert(
                conn_id.to_string(),
                PendingAuth {
                    nonce: nonce.clone(),
                    created_at: Instant::now(),
                },
            );
        }
        self.cleanup_expired().await;
        nonce
    }

    pub async fn verify(
        &self,
        conn_id: &str,
        ty: &str,
        client_id: &str,
        id: &str,
        proof: &str,
    ) -> bool {
        let nonce = {
            let mut pending = self.pending.write().await;
            match pending.remove(conn_id) {
                Some(p) => p.nonce,
                None => return false,
            }
        };
        verify_proof(&self.auth_key, ty, client_id, id, &nonce, proof)
    }

    pub fn compute_proof(&self, ty: &str, client_id: &str, id: &str, nonce: &str) -> String {
        compute_proof(&self.auth_key, ty, client_id, id, nonce)
    }

    pub fn auth_key(&self) -> &str {
        &self.auth_key
    }

    async fn cleanup_expired(&self) {
        let mut pending = self.pending.write().await;
        let now = Instant::now();
        let timeout = Duration::from_secs(60);
        pending.retain(|_, v| now.duration_since(v.created_at) < timeout);
    }
}

#[cfg(test)]
mod tests {
    use super::AuthState;
    use cross233_protocol::crypto::compute_proof;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn challenge_is_created_without_deadlocking_and_is_single_use() {
        let auth = AuthState::new("test-key".to_string());
        let nonce = timeout(
            Duration::from_millis(100),
            auth.create_challenge("client-1"),
        )
        .await
        .expect("challenge creation must not deadlock");
        let proof = compute_proof("test-key", "client", "client-1", "client-1", &nonce);

        assert!(
            auth.verify("client-1", "client", "client-1", "client-1", &proof)
                .await
        );
        assert!(
            !auth
                .verify("client-1", "client", "client-1", "client-1", &proof)
                .await
        );
    }
}
