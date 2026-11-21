use std::{
    fs,
    path::{Path, PathBuf},
};

use agentlink_core::identity::Identity;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{DaemonError, Result};

pub const DEFAULT_DAEMON_PORT: u16 = 47780;
pub const DEFAULT_RELAY_URL: &str = "https://agentlink-relay.workers.dev";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub relay_url: String,
    pub listen_port: u16,
    #[serde(default = "default_responder")]
    pub default_responder: String,
    /// Optional custom shell command for the background responder (useful for custom agents or tests).
    #[serde(default)]
    pub responder_cmd: Option<String>,
}

fn default_responder() -> String {
    "claude".to_string()
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            relay_url: DEFAULT_RELAY_URL.to_string(),
            listen_port: DEFAULT_DAEMON_PORT,
            default_responder: default_responder(),
            responder_cmd: None,
        }
    }
}

/// Manages `~/.agentlink/` (or `AGENTLINK_HOME`).
#[derive(Debug, Clone)]
pub struct AgentLinkHome {
    root: PathBuf,
}

impl AgentLinkHome {
    pub fn resolve() -> Result<Self> {
        let root = if let Ok(custom) = std::env::var("AGENTLINK_HOME") {
            PathBuf::from(custom)
        } else {
            dirs::home_dir()
                .ok_or_else(|| DaemonError::Other("cannot determine home directory".into()))?
                .join(".agentlink")
        };
        Ok(Self { root })
    }

    pub fn from_path(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn identity_path(&self) -> PathBuf {
        self.root.join("identity.json")
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("agentlink.db")
    }

    pub fn token_path(&self) -> PathBuf {
        self.root.join("daemon.token")
    }

    pub fn pid_path(&self) -> PathBuf {
        self.root.join("daemon.pid")
    }

    pub fn is_initialized(&self) -> bool {
        self.identity_path().exists() && self.config_path().exists() && self.token_path().exists()
    }

    /// Initialize `~/.agentlink/` with a fresh identity, config, and local auth token.
    pub fn init(
        &self,
        display_name: &str,
        relay_url: Option<&str>,
        listen_port: Option<u16>,
    ) -> Result<(Identity, DaemonConfig, String)> {
        fs::create_dir_all(&self.root)?;

        let identity = if self.identity_path().exists() {
            self.load_identity()?
        } else {
            let id = Identity::generate(display_name);
            fs::write(self.identity_path(), serde_json::to_string_pretty(&id)?)?;
            id
        };

        let config = if self.config_path().exists() {
            let mut cfg = self.load_config()?;
            if let Some(url) = relay_url {
                cfg.relay_url = url.trim_end_matches('/').to_string();
            }
            if let Some(port) = listen_port {
                cfg.listen_port = port;
            }
            fs::write(self.config_path(), serde_json::to_string_pretty(&cfg)?)?;
            cfg
        } else {
            let cfg = DaemonConfig {
                relay_url: relay_url
                    .unwrap_or(DEFAULT_RELAY_URL)
                    .trim_end_matches('/')
                    .to_string(),
                listen_port: listen_port.unwrap_or(DEFAULT_DAEMON_PORT),
                default_responder: default_responder(),
                responder_cmd: None,
            };
            fs::write(self.config_path(), serde_json::to_string_pretty(&cfg)?)?;
            cfg
        };

        let token = if self.token_path().exists() {
            fs::read_to_string(self.token_path())?.trim().to_string()
        } else {
            let tok = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
            fs::write(self.token_path(), &tok)?;
            tok
        };

        Ok((identity, config, token))
    }

    pub fn load_identity(&self) -> Result<Identity> {
        if !self.identity_path().exists() {
            return Err(DaemonError::NotInitialized);
        }
        let raw = fs::read_to_string(self.identity_path())?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn load_config(&self) -> Result<DaemonConfig> {
        if !self.config_path().exists() {
            return Err(DaemonError::NotInitialized);
        }
        let raw = fs::read_to_string(self.config_path())?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn load_token(&self) -> Result<String> {
        if !self.token_path().exists() {
            return Err(DaemonError::NotInitialized);
        }
        Ok(fs::read_to_string(self.token_path())?.trim().to_string())
    }
}

