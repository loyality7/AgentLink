use std::{
    path::Path,
    process::Stdio,
    sync::Arc,
};

use agentlink_core::{
    crypto::{encrypt_for_peer, EncryptedEnvelope},
    guardrails::{ProjectConfig, ResponderMode},
    identity::Identity,
    protocol::{AgentMessage, MessageType},
};
use serde::{Deserialize, Serialize};
use tokio::{
    process::Command,
    sync::{broadcast, mpsc},
};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{config::DaemonConfig, store::Store};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponderDecision {
    pub action: String,
    pub body: String,
    #[serde(default)]
    pub refs: Vec<String>,
}

/// Best-effort OS desktop notification (never fails the daemon in headless/CI environments).
pub fn notify_human(summary: &str, body: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .summary(&summary)
            .body(&body)
            .appname("AgentLink")
            .show();
    });
}

fn load_project_config(project_root: Option<&str>) -> ProjectConfig {
    if let Some(root) = project_root {
        let cfg_path = Path::new(root).join(".agentlink.json");
        if let Ok(raw) = std::fs::read_to_string(cfg_path) {
            if let Ok(cfg) = serde_json::from_str::<ProjectConfig>(&raw) {
                return cfg;
            }
        }
    }
    ProjectConfig::default()
}

fn build_prompt(msg: &AgentMessage, sender_name: &str, cfg: &ProjectConfig) -> String {
    format!(
        "You are answering a peer coding agent's question on behalf of your user.\n\
         Peer: {sender_name}\n\
         Project: {}\n\
         Allowed paths (read-only): {:?}\n\
         Blocked paths (NEVER read or reveal): {:?}\n\
         Require human confirmation for: {:?}\n\n\
         Question:\n{}\n\n\
         Respond ONLY with a single JSON object:\n\
         {{\"action\": \"answer\" | \"needs_human\", \"body\": \"...\", \"refs\": [\"path/to/file\"]}}",
        cfg.project, cfg.allowed_paths, cfg.blocked_paths, cfg.require_human_for, msg.body
    )
}

async fn invoke_responder_cmd(
    responder_kind: &str,
    custom_cmd: Option<&str>,
    project_root: Option<&str>,
    prompt: &str,
) -> Option<ResponderDecision> {
    let override_cmd = custom_cmd
        .map(str::to_string)
        .or_else(|| std::env::var("AGENTLINK_RESPONDER_CMD").ok());

    let mut cmd = if let Some(ref custom) = override_cmd {
        if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.args(["/C", custom]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", custom]);
            c
        }
    } else {
        match responder_kind {
            "claude" => {
                let mut c = Command::new("claude");
                c.args([
                    "-p",
                    prompt,
                    "--output-format",
                    "text",
                    "--allowedTools",
                    "Read,Glob,Grep",
                ]);
                c
            }
            "codex" => {
                let mut c = Command::new("codex");
                c.args(["exec", "--sandbox", "read-only", prompt]);
                c
            }
            _ => return None,
        }
    };

    if let Some(root) = project_root {
        if Path::new(root).is_dir() {
            cmd.current_dir(root);
        }
    }

    cmd.env("AGENTLINK_PROMPT", prompt)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = match tokio::time::timeout(std::time::Duration::from_secs(45), cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => out,
        Ok(Ok(out)) => {
            debug!(
                "responder exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
            return None;
        }
        Ok(Err(e)) => {
            debug!("failed to spawn responder '{responder_kind}': {e}");
            return None;
        }
        Err(_) => {
            warn!("background responder timed out");
            return None;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if let Ok(decision) = serde_json::from_str::<ResponderDecision>(trimmed) {
        return Some(decision);
    }
    // Extract JSON object if wrapped in markdown fences or prose
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            if let Ok(decision) = serde_json::from_str::<ResponderDecision>(&trimmed[start..=end]) {
                return Some(decision);
            }
        }
    }
    None
}

/// Handle an incoming remote message in the background:
/// auto-answer if permitted and answerable, or escalate to human.
pub async fn maybe_auto_respond(
    identity: Identity,
    config: DaemonConfig,
    store: Arc<Store>,
    tx_out: mpsc::Sender<EncryptedEnvelope>,
    notify_thread: broadcast::Sender<String>,
    sender_name: String,
    msg: AgentMessage,
) {
    if !matches!(msg.msg_type, MessageType::Question | MessageType::ClarificationNeeded) {
        return;
    }

    let peer = match store.get_peer(&sender_name) {
        Ok(Some(p)) => p,
        _ => return,
    };

    let proj_cfg = load_project_config(peer.project.as_deref());

    // Enforce hop limits and AskHuman mode
    if !msg.can_auto_reply() || proj_cfg.mode == ResponderMode::AskHuman {
        let _ = store.mark_thread_needs_human(&msg.thread_id);
        notify_human(
            &format!("AgentLink: Question from {sender_name}"),
            &msg.body,
        );
        let _ = send_reply(
            &identity,
            &store,
            &tx_out,
            &notify_thread,
            &peer.name,
            &msg,
            MessageType::PendingHuman,
            true,
            "Waiting for human confirmation.".to_string(),
            Vec::new(),
        )
        .await;
        return;
    }

    let prompt = build_prompt(&msg, &sender_name, &proj_cfg);
    let responder_kind = if !proj_cfg.responder.is_empty() {
        proj_cfg.responder.as_str()
    } else {
        config.default_responder.as_str()
    };

    let Some(decision) = invoke_responder_cmd(
        responder_kind,
        config.responder_cmd.as_deref(),
        peer.project.as_deref(),
        &prompt,
    )
    .await
    else {
        // Responder unavailable or not configured; leave unread for active session / user
        return;
    };

    if decision.action == "needs_human" {
        let _ = store.mark_thread_needs_human(&msg.thread_id);
        notify_human(
            &format!("AgentLink: Needs your input ({sender_name})"),
            &decision.body,
        );
        let _ = send_reply(
            &identity,
            &store,
            &tx_out,
            &notify_thread,
            &peer.name,
            &msg,
            MessageType::PendingHuman,
            true,
            decision.body,
            decision.refs,
        )
        .await;
    } else {
        info!("background responder auto-answered thread {}", msg.thread_id);
        let _ = store.mark_read(&msg.id);
        let _ = send_reply(
            &identity,
            &store,
            &tx_out,
            &notify_thread,
            &peer.name,
            &msg,
            MessageType::Answer,
            false,
            decision.body,
            decision.refs,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_reply(
    identity: &Identity,
    store: &Arc<Store>,
    tx_out: &mpsc::Sender<EncryptedEnvelope>,
    notify_thread: &broadcast::Sender<String>,
    peer_name: &str,
    incoming: &AgentMessage,
    msg_type: MessageType,
    requires_human: bool,
    body: String,
    refs: Vec<String>,
) -> crate::error::Result<()> {
    let peer = store
        .get_peer(peer_name)?
        .ok_or_else(|| crate::error::DaemonError::PeerNotFound(peer_name.to_string()))?;

    let msg_id = format!("msg_{}", Uuid::new_v4().simple());
    let reply_msg = AgentMessage {
        v: 1,
        id: msg_id.clone(),
        thread_id: incoming.thread_id.clone(),
        reply_to: Some(incoming.id.clone()),
        msg_type,
        project: incoming.project.clone(),
        hop: incoming.hop.saturating_add(1),
        max_hops: incoming.max_hops,
        requires_human,
        body,
        refs,
        group: None,
    };

    let plaintext = serde_json::to_vec(&reply_msg)?;
    let env = encrypt_for_peer(identity, &peer.to_public_identity(), &msg_id, &plaintext)?;

    store.insert_message(&reply_msg, "auto-responder", &peer.name, "out", "read")?;
    store.enqueue_outbox(&env)?;
    let _ = tx_out.send(env).await;
    let _ = notify_thread.send(incoming.thread_id.clone());
    Ok(())
}
