pub mod cost;
pub mod dataset;
pub mod evidence;
pub mod grade;
pub mod instructions;
pub mod provider;
pub mod report;
pub mod run;

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
