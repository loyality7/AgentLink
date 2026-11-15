pub mod crypto;
pub mod guardrails;
pub mod identity;
pub mod protocol;

use thiserror::Error;

/// Errors returned by `agentlink-core` operations.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid key material: {0}")]
    InvalidKey(String),

    #[error("peer id mismatch: expected {expected}, got {actual}")]
    PeerIdMismatch { expected: String, actual: String },

    #[error("signature verification failed")]
    InvalidSignature,

    #[error("encryption failed: {0}")]
    Encryption(String),

    #[error("decryption failed")]
    Decryption,

    #[error("invalid pairing token: {0}")]
    InvalidPairingToken(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("glob pattern error: {0}")]
    Glob(#[from] globset::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;

