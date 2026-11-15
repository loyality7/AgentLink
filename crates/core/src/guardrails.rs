use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::Result;

/// Per-project guardrails stored in `<repo>/.agentlink.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project: String,
    #[serde(default = "default_mode")]
    pub mode: ResponderMode,
    #[serde(default = "default_responder")]
    pub responder: String,
    #[serde(default = "default_allowed_paths")]
    pub allowed_paths: Vec<String>,
    #[serde(default = "default_blocked_paths")]
    pub blocked_paths: Vec<String>,
    #[serde(default = "default_max_hops")]
    pub max_hops: u8,
    #[serde(default = "default_require_human_for")]
    pub require_human_for: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponderMode {
    Auto,
    AskHuman,
}

fn default_mode() -> ResponderMode {
    ResponderMode::Auto
}
fn default_responder() -> String {
    "claude".to_string()
}
fn default_allowed_paths() -> Vec<String> {
    vec!["src/**".into(), "docs/**".into(), "README.md".into(), "PLAN.md".into()]
}
fn default_blocked_paths() -> Vec<String> {
    vec![
        ".env*".into(),
        "**/.env*".into(),
        "**/*.pem".into(),
        "**/*.key".into(),
        "**/secrets/**".into(),
    ]
}
fn default_max_hops() -> u8 {
    5
}
fn default_require_human_for() -> Vec<String> {
    vec![
        "architecture changes".into(),
        "modifying or deleting files".into(),
        "sharing credentials or API keys".into(),
    ]
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            project: "default".to_string(),
            mode: default_mode(),
            responder: default_responder(),
            allowed_paths: default_allowed_paths(),
            blocked_paths: default_blocked_paths(),
            max_hops: default_max_hops(),
            require_human_for: default_require_human_for(),
        }
    }
}

/// Compiled path matcher that enforces `blocked_paths` before `allowed_paths`.
pub struct PathGuard {
    allowed: GlobSet,
    blocked: GlobSet,
}

impl PathGuard {
    pub fn from_config(cfg: &ProjectConfig) -> Result<Self> {
        let mut allowed_builder = GlobSetBuilder::new();
        for pat in &cfg.allowed_paths {
            allowed_builder.add(Glob::new(pat)?);
        }

        let mut blocked_builder = GlobSetBuilder::new();
        for pat in &cfg.blocked_paths {
            blocked_builder.add(Glob::new(pat)?);
        }

        Ok(Self {
            allowed: allowed_builder.build()?,
            blocked: blocked_builder.build()?,
        })
    }

    /// Returns true only if `rel_path` matches `allowed_paths` and does NOT match `blocked_paths`.
    pub fn is_allowed(&self, rel_path: &str) -> bool {
        let normalized = rel_path.replace('\\', "/");
        if self.blocked.is_match(&normalized) {
            return false;
        }
        self.allowed.is_match(&normalized)
    }
}

