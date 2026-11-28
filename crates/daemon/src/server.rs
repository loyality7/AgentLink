use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use agentlink_core::{
    crypto::{encrypt_for_peer, EncryptedEnvelope},
    identity::Identity,
    protocol::{AgentMessage, GroupMeta, MessageType},
};
use axum::{
    extract::{Query, Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, RwLock};
use uuid::Uuid;

use crate::{
    config::DaemonConfig,
    store::{Store, StoredGroup, StoredMessage, StoredPeer},
};

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalSession {
    pub session_id: String,
    pub name: String,
    pub agent_kind: String,
    pub project_root: String,
    pub last_seen: i64,
}

#[derive(Clone)]
pub struct AppState {
    pub identity: Identity,
    pub config: DaemonConfig,
    pub token: String,
    pub store: Arc<Store>,
    pub tx_out: mpsc::Sender<EncryptedEnvelope>,
    pub sessions: Arc<RwLock<HashMap<String, LocalSession>>>,
    pub notify_thread: broadcast::Sender<String>,
}

pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/health", get(health))
        .route("/sessions/register", post(register_session))
        .route("/peers", get(list_peers))
        .route("/ask", post(ask_peer))
        .route("/reply", post(reply_peer))
        .route("/inbox", get(get_inbox))
        .route("/inbox/ack", post(ack_message))
        .route("/escalate", post(escalate_thread))
        .route("/groups", get(list_groups).post(create_group))
        .route("/groups/members", post(update_group_members))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
        .with_state(state);
    protected
}

async fn auth_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> std::result::Result<Response, StatusCode> {
    let expected = format!("Bearer {}", state.token);
    let header_val = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if header_val != Some(expected.as_str()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(req).await)
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "peer_id": state.identity.peer_id,
        "display_name": state.identity.display_name,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RegisterSessionReq {
    pub session_id: Option<String>,
    pub name: String,
    pub agent_kind: String,
    pub project_root: String,
}

async fn register_session(
    State(state): State<AppState>,
    Json(req): Json<RegisterSessionReq>,
) -> Json<LocalSession> {
    let sid = req
        .session_id
        .unwrap_or_else(|| format!("sess_{}", Uuid::new_v4().simple()));
    let session = LocalSession {
        session_id: sid.clone(),
        name: req.name,
        agent_kind: req.agent_kind,
        project_root: req.project_root,
        last_seen: now_unix(),
    };
    state.sessions.write().await.insert(sid, session.clone());
    Json(session)
}

#[derive(Debug, Serialize)]
pub struct PeersResponse {
    pub remote: Vec<StoredPeer>,
    pub local: Vec<LocalSession>,
    pub groups: Vec<StoredGroup>,
}

const SESSION_TTL_SECS: i64 = 3600;

async fn prune_stale_sessions(state: &AppState) {
    let cutoff = now_unix() - SESSION_TTL_SECS;
    state
        .sessions
        .write()
        .await
        .retain(|_, s| s.last_seen >= cutoff);
}

async fn list_peers(
    State(state): State<AppState>,
) -> std::result::Result<Json<PeersResponse>, (StatusCode, String)> {
    prune_stale_sessions(&state).await;
    let remote = state
        .store
        .list_peers()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let local: Vec<LocalSession> = state.sessions.read().await.values().cloned().collect();
    let groups = state
        .store
        .list_groups()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(PeersResponse { remote, local, groups }))
}

#[derive(Debug, Deserialize)]
pub struct AskReq {
    pub target: String,
    pub question: String,
    pub thread_id: Option<String>,
    pub project: Option<String>,
    pub wait_seconds: Option<u64>,
    pub from_session: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AskResp {
    pub status: String,
    pub thread_id: String,
    pub msg_id: String,
    pub reply: Option<StoredMessage>,
}

async fn resolve_local_session(state: &AppState, target: &str) -> Option<LocalSession> {
    prune_stale_sessions(state).await;
    let lookup = target.strip_prefix("local:").unwrap_or(target);
    let guard = state.sessions.read().await;
    guard
        .values()
        .find(|s| s.name == lookup || s.session_id == lookup)
        .cloned()
}

async fn fanout_to_group(
    state: &AppState,
    group: &StoredGroup,
    base_msg: &AgentMessage,
    from_label: &str,
) -> std::result::Result<(), (StatusCode, String)> {
    let mut out_record = base_msg.clone();
    out_record.id = format!("{}:out", base_msg.id);
    state
        .store
        .insert_message(
            &out_record,
            from_label,
            &format!("group:{}", group.name),
            "out",
            "read",
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    for member in &group.members {
        if member == &state.identity.peer_id {
            continue;
        }
        if let Some(local_target) = resolve_local_session(state, member).await {
            if local_target.name == from_label {
                continue;
            }
            let local_target_key = format!("local:{}", local_target.name);
            let mut local_msg = base_msg.clone();
            local_msg.id = format!("{}:local:{}", base_msg.id, local_target.name);
            state
                .store
                .insert_message(
                    &local_msg,
                    &format!("local:{from_label}"),
                    &local_target_key,
                    "in",
                    "unread",
                )
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        } else if let Some(remote_peer) = state
            .store
            .get_peer(member)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            let mut peer_msg = base_msg.clone();
            peer_msg.id = format!("{}:{}", base_msg.id, &remote_peer.peer_id[..8]);
            // Strip local-only session names before sending GroupMeta over the relay
            if let Some(ref mut g) = peer_msg.group {
                g.members.retain(|m| !m.starts_with("local:"));
            }
            let plaintext = serde_json::to_vec(&peer_msg)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let env = encrypt_for_peer(
                &state.identity,
                &remote_peer.to_public_identity(),
                &peer_msg.id,
                &plaintext,
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            state
                .store
                .enqueue_outbox(&env)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let _ = state.tx_out.send(env).await;
        }
    }
    let _ = state.notify_thread.send(base_msg.thread_id.clone());
    Ok(())
}

async fn ask_peer(
    State(state): State<AppState>,
    Json(req): Json<AskReq>,
) -> std::result::Result<Json<AskResp>, (StatusCode, String)> {
    let msg_id = format!("msg_{}", Uuid::new_v4().simple());
    let thread_id = req
        .thread_id
        .unwrap_or_else(|| format!("thr_{}", Uuid::new_v4().simple()));
    let from_label = req
        .from_session
        .clone()
        .unwrap_or_else(|| "self".to_string());

    let mut msg = AgentMessage::new_question(&msg_id, &thread_id, req.project.clone(), req.question);

    let mut rx_notify = state.notify_thread.subscribe();

    if let Some(group) = state
        .store
        .get_group(&req.target)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        msg.group = Some(GroupMeta {
            group_id: group.group_id.clone(),
            name: group.name.clone(),
            members: group.members.clone(),
        });
        fanout_to_group(&state, &group, &msg, &from_label).await?;
    } else if let Some(local_target) = resolve_local_session(&state, &req.target).await {
        // Local same-laptop routing: zero relay traffic!
        let local_target_key = format!("local:{}", local_target.name);
        let mut out_msg = msg.clone();
        out_msg.id = format!("{msg_id}:out");
        state
            .store
            .insert_message(&out_msg, &from_label, &local_target_key, "out", "read")
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        state
            .store
            .insert_message(&msg, &format!("local:{from_label}"), &local_target_key, "in", "unread")
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let _ = state.notify_thread.send(thread_id.clone());
        tokio::spawn(crate::responder::maybe_auto_respond_local(
            state.config.clone(),
            Arc::clone(&state.store),
            state.notify_thread.clone(),
            from_label.clone(),
            local_target.name.clone(),
            local_target.project_root.clone(),
            msg.clone(),
        ));
    } else if let Some(remote_peer) = state
        .store
        .get_peer(&req.target)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let plaintext = serde_json::to_vec(&msg)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let env = encrypt_for_peer(
            &state.identity,
            &remote_peer.to_public_identity(),
            &msg_id,
            &plaintext,
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        state
            .store
            .insert_message(&msg, &from_label, &remote_peer.name, "out", "read")
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        state
            .store
            .enqueue_outbox(&env)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let _ = state.tx_out.send(env).await;
    } else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("peer or local session '{}' not found", req.target),
        ));
    }

    let wait_secs = req.wait_seconds.unwrap_or(0);
    if wait_secs > 0 {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_secs);
        loop {
            if let Ok(Some(reply)) = state.store.latest_reply_in_thread(&thread_id, &msg_id) {
                return Ok(Json(AskResp {
                    status: "answered".to_string(),
                    thread_id,
                    msg_id,
                    reply: Some(reply),
                }));
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let _ = tokio::time::timeout(remaining, rx_notify.recv()).await;
        }
    }

    Ok(Json(AskResp {
        status: "pending".to_string(),
        thread_id,
        msg_id,
        reply: None,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ReplyReq {
    pub thread_id: String,
    pub body: String,
    pub requires_human: Option<bool>,
    pub refs: Option<Vec<String>>,
    pub from_session: Option<String>,
}

async fn reply_peer(
    State(state): State<AppState>,
    Json(req): Json<ReplyReq>,
) -> std::result::Result<Json<serde_json::Value>, (StatusCode, String)> {
    let incoming = state
        .store
        .latest_incoming_in_thread(&req.thread_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("thread '{}' not found", req.thread_id)))?;

    let msg_id = format!("msg_{}", Uuid::new_v4().simple());
    let requires_human = req.requires_human.unwrap_or(false);
    let mut reply_msg = AgentMessage {
        v: 1,
        id: msg_id.clone(),
        thread_id: req.thread_id.clone(),
        reply_to: Some(incoming.id.clone()),
        msg_type: if requires_human {
            MessageType::PendingHuman
        } else {
            MessageType::Answer
        },
        project: incoming.project.clone(),
        hop: incoming.hop.saturating_add(1),
        max_hops: incoming.max_hops,
        requires_human,
        body: req.body,
        refs: req.refs.unwrap_or_default(),
        group: None,
    };

    let from_label = req
        .from_session
        .clone()
        .unwrap_or_else(|| "self".to_string());

    if let Some(ref gid) = incoming.group_id {
        if let Some(group) = state
            .store
            .get_group(gid)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            reply_msg.group = Some(GroupMeta {
                group_id: group.group_id.clone(),
                name: group.name.clone(),
                members: group.members.clone(),
            });
            let _ = state.store.mark_read(&incoming.id);
            fanout_to_group(&state, &group, &reply_msg, &from_label).await?;
            return Ok(Json(serde_json::json!({
                "ok": true,
                "msg_id": msg_id,
                "thread_id": req.thread_id,
            })));
        }
    }

    if incoming.from_peer.starts_with("local:") {
        let target_session = incoming.from_peer.clone();
        state
            .store
            .insert_message(&reply_msg, &format!("local:{from_label}"), &target_session, "in", "unread")
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let _ = state.store.mark_read(&incoming.id);
        let _ = state.notify_thread.send(req.thread_id.clone());
    } else {
        let peer = state
            .store
            .get_peer(&incoming.from_peer)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or_else(|| (StatusCode::NOT_FOUND, format!("peer '{}' not found", incoming.from_peer)))?;

        let plaintext = serde_json::to_vec(&reply_msg)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let env = encrypt_for_peer(&state.identity, &peer.to_public_identity(), &msg_id, &plaintext)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        state
            .store
            .insert_message(&reply_msg, &from_label, &peer.name, "out", "read")
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let _ = state.store.mark_read(&incoming.id);
        state
            .store
            .enqueue_outbox(&env)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let _ = state.tx_out.send(env).await;
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "msg_id": msg_id,
        "thread_id": req.thread_id,
    })))
}

#[derive(Debug, Deserialize)]
pub struct InboxQuery {
    pub thread_id: Option<String>,
    pub unread: Option<u8>,
    pub session: Option<String>,
}

async fn get_inbox(
    State(state): State<AppState>,
    Query(q): Query<InboxQuery>,
) -> std::result::Result<Json<Vec<StoredMessage>>, (StatusCode, String)> {
    let unread_only = q.unread.unwrap_or(0) != 0;
    let to_filter = q.session.map(|s| {
        if s.starts_with("local:") {
            s
        } else {
            format!("local:{s}")
        }
    });
    let msgs = state
        .store
        .list_messages(q.thread_id.as_deref(), unread_only, to_filter.as_deref())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(msgs))
}

#[derive(Debug, Deserialize)]
pub struct AckReq {
    pub msg_id: String,
}

async fn ack_message(
    State(state): State<AppState>,
    Json(req): Json<AckReq>,
) -> std::result::Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .store
        .mark_read(&req.msg_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct EscalateReq {
    pub thread_id: String,
    pub reason: String,
}

async fn escalate_thread(
    State(state): State<AppState>,
    Json(req): Json<EscalateReq>,
) -> std::result::Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .store
        .mark_thread_needs_human(&req.thread_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::responder::notify_human("AgentLink: Needs your confirmation", &req.reason);
    Ok(Json(serde_json::json!({
        "ok": true,
        "thread_id": req.thread_id,
        "status": "needs_human",
        "reason": req.reason,
    })))
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupReq {
    pub name: String,
    pub members: Vec<String>,
}

async fn create_group(
    State(state): State<AppState>,
    Json(req): Json<CreateGroupReq>,
) -> std::result::Result<Json<StoredGroup>, (StatusCode, String)> {
    let group_id = format!("grp_{}", Uuid::new_v4().simple());
    let mut members: Vec<String> = Vec::new();
    for m in req.members {
        if m.starts_with("local:") {
            if !members.contains(&m) {
                members.push(m);
            }
        } else if let Some(peer) = state
            .store
            .get_peer(&m)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            if !members.contains(&peer.peer_id) {
                members.push(peer.peer_id);
            }
        } else {
            return Err((StatusCode::NOT_FOUND, format!("peer '{m}' not found")));
        }
    }
    if !members.contains(&state.identity.peer_id) {
        members.push(state.identity.peer_id.clone());
    }
    state
        .store
        .upsert_group(&group_id, &req.name, &members)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let stored = state
        .store
        .get_group(&group_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "group missing".into()))?;
    Ok(Json(stored))
}

async fn list_groups(
    State(state): State<AppState>,
) -> std::result::Result<Json<Vec<StoredGroup>>, (StatusCode, String)> {
    let groups = state
        .store
        .list_groups()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(groups))
}

#[derive(Debug, Deserialize)]
pub struct UpdateGroupMembersReq {
    pub group: String,
    pub add: Option<String>,
    pub remove: Option<String>,
}

async fn update_group_members(
    State(state): State<AppState>,
    Json(req): Json<UpdateGroupMembersReq>,
) -> std::result::Result<Json<StoredGroup>, (StatusCode, String)> {
    let mut group = state
        .store
        .get_group(&req.group)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, format!("group '{}' not found", req.group)))?;

    if let Some(add_m) = req.add {
        let resolved = if add_m.starts_with("local:") {
            add_m
        } else {
            state
                .store
                .get_peer(&add_m)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .map(|p| p.peer_id)
                .ok_or((StatusCode::NOT_FOUND, format!("peer '{add_m}' not found")))?
        };
        if !group.members.contains(&resolved) {
            group.members.push(resolved);
        }
    }
    if let Some(rem_m) = req.remove {
        let resolved = state
            .store
            .get_peer(&rem_m)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .map(|p| p.peer_id)
            .unwrap_or(rem_m);
        group.members.retain(|m| m != &resolved);
    }
    state
        .store
        .upsert_group(&group.group_id, &group.name, &group.members)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(group))
}

