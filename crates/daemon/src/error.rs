use thiserror::Error;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("not initialized; run `agentlink init` first")]
    NotInitialized,

    #[error("core error: {0}")]
    Core(#[from] agentlink_core::CoreError),

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("peer not found: {0}")]
    PeerNotFound(String),

    #[error("thread not found: {0}")]
    ThreadNotFound(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, DaemonError>;

