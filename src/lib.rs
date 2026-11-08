pub mod batch;
pub mod cost;
pub mod credentials;
pub mod dataset;
pub mod evidence;
pub mod grade;
pub mod instructions;
pub mod provider;
pub mod report;
pub mod run;
pub mod score;
pub mod tui;

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
