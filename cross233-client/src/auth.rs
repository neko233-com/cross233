use cross233_protocol::crypto;

pub fn compute_proof(key: &str, ty: &str, client_id: &str, id: &str, nonce: &str) -> String {
    crypto::compute_proof(key, ty, client_id, id, nonce)
}

pub fn generate_nonce() -> String {
    crypto::generate_nonce()
}
