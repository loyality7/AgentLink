use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use agentlink_core::{
    crypto::EncryptedEnvelope,
    protocol::{generate_pairing_code, PairingBundle, RelayFrame},
};
use agentlink_daemon::{
    config::AgentLinkHome,
    relay_client::{fetch_pairing, publish_pairing},
    start_daemon,
    store::{Store, StoredMessage},
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path as AxumPath, Query, State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tempfile::TempDir;
use tokio::{
    net::TcpListener,
    sync::{mpsc, Mutex},
};

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Default)]
struct MockRelayState {
    connections: Mutex<HashMap<String, mpsc::UnboundedSender<RelayFrame>>>,
    offline_queue: Mutex<HashMap<String, Vec<EncryptedEnvelope>>>,
    pairing: Mutex<HashMap<String, String>>,
}

async fn start_mock_relay() -> SocketAddr {
    let state = Arc::new(MockRelayState::default());
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/pair", post(pair_post))
        .route("/pair/{code}", get(pair_get))
        .with_state(state);

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

#[derive(Deserialize)]
struct WsQuery {
    peer_id: String,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(state): State<Arc<MockRelayState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, q.peer_id, state))
}

async fn handle_ws(socket: WebSocket, _peer_id: String, state: Arc<MockRelayState>) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<RelayFrame>();

    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let txt = serde_json::to_string(&frame).unwrap();
            if sender.send(Message::Text(txt.into())).await.is_err() {
                break;
            }
        }
    });

    let mut authed_peer: Option<String> = None;

    while let Some(Ok(Message::Text(txt))) = receiver.next().await {
        let Ok(frame) = serde_json::from_str::<RelayFrame>(&txt) else {
            continue;
        };
        match frame {
            RelayFrame::Auth { peer_id, .. } => {
                authed_peer = Some(peer_id.clone());
                state.connections.lock().await.insert(peer_id.clone(), tx.clone());
                let _ = tx.send(RelayFrame::AuthOk {
                    peer_id: peer_id.clone(),
                });
                let pending = state
                    .offline_queue
                    .lock()
                    .await
                    .remove(&peer_id)
                    .unwrap_or_default();
                for env in pending {
                    let _ = tx.send(RelayFrame::Deliver { envelope: env });
                }
            }
            RelayFrame::Send { envelope } => {
                let target = envelope.to.clone();
                if let Some(target_tx) = state.connections.lock().await.get(&target).cloned() {
                    let _ = target_tx.send(RelayFrame::Deliver { envelope });
                } else {
                    state
                        .offline_queue
                        .lock()
                        .await
                        .entry(target)
                        .or_default()
                        .push(envelope);
                }
            }
            RelayFrame::Ack { .. } => {}
            _ => {}
        }
    }

    if let Some(pid) = authed_peer {
        state.connections.lock().await.remove(&pid);
    }
    writer.abort();
}

#[derive(Deserialize)]
struct PairPostReq {
    code: String,
    payload: String,
}

async fn pair_post(
    State(state): State<Arc<MockRelayState>>,
    Json(req): Json<PairPostReq>,
) -> StatusCode {
    state.pairing.lock().await.insert(req.code, req.payload);
    StatusCode::OK
}

async fn pair_get(
    AxumPath(code): AxumPath<String>,
    State(state): State<Arc<MockRelayState>>,
) -> Result<String, StatusCode> {
    state
        .pairing
        .lock()
        .await
        .get(&code)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)
}

async fn pair_two_homes(
    relay_url: &str,
    alice_home: &AgentLinkHome,
    bob_home: &AgentLinkHome,
) {
    let alice_id = alice_home.load_identity().unwrap();
    let bob_id = bob_home.load_identity().unwrap();
    let code = generate_pairing_code();

    publish_pairing(
        relay_url,
        &PairingBundle {
            v: 1,
            code: code.clone(),
            peer: alice_id.to_public(),
            project: Some("shared-app".into()),
            expires_at: now_unix() + 600,
        },
    )
    .await
    .unwrap();

    let fetched_alice = fetch_pairing(relay_url, &code).await.unwrap().unwrap();
    Store::open(&bob_home.db_path())
        .unwrap()
        .upsert_peer(&fetched_alice.peer, "alice", fetched_alice.project.as_deref())
        .unwrap();

    publish_pairing(
        relay_url,
        &PairingBundle {
            v: 1,
            code: format!("{code}:ack"),
            peer: bob_id.to_public(),
            project: Some("shared-app".into()),
            expires_at: now_unix() + 600,
        },
    )
    .await
    .unwrap();

    let fetched_bob = fetch_pairing(relay_url, &format!("{code}:ack"))
        .await
        .unwrap()
        .unwrap();
    Store::open(&alice_home.db_path())
        .unwrap()
        .upsert_peer(&fetched_bob.peer, "bob", fetched_bob.project.as_deref())
        .unwrap();
}

#[tokio::test]
async fn sim_remote_peers_realtime_ask_and_reply() {
    let relay_addr = start_mock_relay().await;
    let relay_url = format!("http://{relay_addr}");

    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let alice_home = AgentLinkHome::from_path(alice_dir.path());
    let bob_home = AgentLinkHome::from_path(bob_dir.path());

    alice_home.init("alice", Some(&relay_url), Some(0)).unwrap();
    bob_home.init("bob", Some(&relay_url), Some(0)).unwrap();
    pair_two_homes(&relay_url, &alice_home, &bob_home).await;

    let alice_d = start_daemon(&alice_home, Some(0)).await.unwrap();
    let bob_d = start_daemon(&bob_home, Some(0)).await.unwrap();

    // Wait briefly for both daemons to authenticate with mock relay
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Simulate Bob's background agent polling for questions and answering
    let bob_addr = bob_d.addr;
    let bob_tok = bob_d.token.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        for _ in 0..40 {
            let msgs: Vec<StoredMessage> = client
                .get(format!("http://{bob_addr}/inbox?unread=1"))
                .bearer_auth(&bob_tok)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if let Some(q) = msgs.first() {
                client
                    .post(format!("http://{bob_addr}/reply"))
                    .bearer_auth(&bob_tok)
                    .json(&serde_json::json!({
                        "thread_id": q.thread_id,
                        "body": "Use { code: string, message: string }",
                        "refs": ["src/orders/errors.ts"]
                    }))
                    .send()
                    .await
                    .unwrap();
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    let alice_client = reqwest::Client::new();
    let ask_resp: serde_json::Value = alice_client
        .post(format!("http://{}/ask", alice_d.addr))
        .bearer_auth(&alice_d.token)
        .json(&serde_json::json!({
            "target": "bob",
            "question": "What is the /orders error response format?",
            "wait_seconds": 5
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(ask_resp["status"], "answered");
    assert_eq!(
        ask_resp["reply"]["body"],
        "Use { code: string, message: string }"
    );

    let _ = alice_d.shutdown.send(true);
    let _ = bob_d.shutdown.send(true);
}

#[tokio::test]
async fn sim_offline_queue_delivers_when_peer_comes_online() {
    let relay_addr = start_mock_relay().await;
    let relay_url = format!("http://{relay_addr}");

    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let alice_home = AgentLinkHome::from_path(alice_dir.path());
    let bob_home = AgentLinkHome::from_path(bob_dir.path());

    alice_home.init("alice", Some(&relay_url), Some(0)).unwrap();
    bob_home.init("bob", Some(&relay_url), Some(0)).unwrap();
    pair_two_homes(&relay_url, &alice_home, &bob_home).await;

    // Only Alice is online; Bob is offline
    let alice_d = start_daemon(&alice_home, Some(0)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let alice_client = reqwest::Client::new();
    let ask_resp: serde_json::Value = alice_client
        .post(format!("http://{}/ask", alice_d.addr))
        .bearer_auth(&alice_d.token)
        .json(&serde_json::json!({
            "target": "bob",
            "question": "Which port does the auth service use?",
            "wait_seconds": 0
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ask_resp["status"], "pending");

    tokio::time::sleep(Duration::from_millis(150)).await;

    // Now Bob comes online and should receive the queued encrypted message
    let bob_d = start_daemon(&bob_home, Some(0)).await.unwrap();
    let bob_client = reqwest::Client::new();

    let mut received = Vec::new();
    for _ in 0..30 {
        received = bob_client
            .get(format!("http://{}/inbox?unread=1", bob_d.addr))
            .bearer_auth(&bob_d.token)
            .send()
            .await
            .unwrap()
            .json::<Vec<StoredMessage>>()
            .await
            .unwrap();
        if !received.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert_eq!(received.len(), 1);
    assert_eq!(received[0].body, "Which port does the auth service use?");

    let _ = alice_d.shutdown.send(true);
    let _ = bob_d.shutdown.send(true);
}

#[tokio::test]
async fn sim_two_local_agents_on_same_laptop() {
    let home_dir = TempDir::new().unwrap();
    let home = AgentLinkHome::from_path(home_dir.path());
    home.init("me", Some("http://127.0.0.1:9"), Some(0)).unwrap();

    let daemon = start_daemon(&home, Some(0)).await.unwrap();
    let client = reqwest::Client::new();

    // Register two local sessions: backend (Claude) and frontend (Codex)
    for (name, kind, root) in [
        ("backend", "claude", "/repo/backend"),
        ("frontend", "codex", "/repo/frontend"),
    ] {
        client
            .post(format!("http://{}/sessions/register", daemon.addr))
            .bearer_auth(&daemon.token)
            .json(&serde_json::json!({
                "name": name,
                "agent_kind": kind,
                "project_root": root
            }))
            .send()
            .await
            .unwrap();
    }

    // Frontend agent polls its local inbox and replies
    let d_addr = daemon.addr;
    let d_tok = daemon.token.clone();
    tokio::spawn(async move {
        let c = reqwest::Client::new();
        for _ in 0..30 {
            let msgs: Vec<StoredMessage> = c
                .get(format!("http://{d_addr}/inbox?unread=1&session=frontend"))
                .bearer_auth(&d_tok)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if let Some(q) = msgs.first() {
                c.post(format!("http://{d_addr}/reply"))
                    .bearer_auth(&d_tok)
                    .json(&serde_json::json!({
                        "thread_id": q.thread_id,
                        "body": "Frontend expects Bearer token in Authorization header",
                        "from_session": "frontend"
                    }))
                    .send()
                    .await
                    .unwrap();
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    });

    let ask_resp: serde_json::Value = client
        .post(format!("http://{}/ask", daemon.addr))
        .bearer_auth(&daemon.token)
        .json(&serde_json::json!({
            "target": "local:frontend",
            "question": "How do you send the auth token?",
            "from_session": "backend",
            "wait_seconds": 3
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(ask_resp["status"], "answered");
    assert_eq!(
        ask_resp["reply"]["body"],
        "Frontend expects Bearer token in Authorization header"
    );

    let _ = daemon.shutdown.send(true);
}

#[tokio::test]
async fn sim_auth_and_human_escalation() {
    let home_dir = TempDir::new().unwrap();
    let home = AgentLinkHome::from_path(home_dir.path());
    home.init("me", Some("http://127.0.0.1:9"), Some(0)).unwrap();
    let daemon = start_daemon(&home, Some(0)).await.unwrap();
    let client = reqwest::Client::new();

    // Unauthorized request must fail with 401
    let unauth = client
        .get(format!("http://{}/health", daemon.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Escalate a thread
    let esc: serde_json::Value = client
        .post(format!("http://{}/escalate", daemon.addr))
        .bearer_auth(&daemon.token)
        .json(&serde_json::json!({
            "thread_id": "thr_decision_1",
            "reason": "Needs human approval to change DB schema"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(esc["status"], "needs_human");

    let _ = daemon.shutdown.send(true);
}

fn write_mock_responder(dir: &TempDir, json_output: &str) -> String {
    if cfg!(target_os = "windows") {
        let path = dir.path().join("mock_responder.cmd");
        std::fs::write(&path, format!("@echo off\r\necho {json_output}\r\n")).unwrap();
        path.to_string_lossy().to_string()
    } else {
        let path = dir.path().join("mock_responder.sh");
        std::fs::write(&path, format!("#!/usr/bin/env sh\nprintf '%s\\n' '{json_output}'\n"))
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path.to_string_lossy().to_string()
    }
}

#[tokio::test]
async fn sim_background_responder_auto_answers() {
    let relay_addr = start_mock_relay().await;
    let relay_url = format!("http://{relay_addr}");

    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let alice_home = AgentLinkHome::from_path(alice_dir.path());
    let bob_home = AgentLinkHome::from_path(bob_dir.path());

    alice_home.init("alice", Some(&relay_url), Some(0)).unwrap();
    bob_home.init("bob", Some(&relay_url), Some(0)).unwrap();

    // Configure Bob's daemon with a mock headless responder that auto-answers
    let mut bob_cfg = bob_home.load_config().unwrap();
    bob_cfg.responder_cmd = Some(write_mock_responder(
        &bob_dir,
        r#"{"action":"answer","body":"JWT expires in 15m","refs":["src/auth.rs"]}"#,
    ));
    std::fs::write(
        bob_home.config_path(),
        serde_json::to_string_pretty(&bob_cfg).unwrap(),
    )
    .unwrap();

    pair_two_homes(&relay_url, &alice_home, &bob_home).await;

    let alice_d = start_daemon(&alice_home, Some(0)).await.unwrap();
    let bob_d = start_daemon(&bob_home, Some(0)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Alice asks Bob; nobody on Bob's side polls manually—Bob's background responder answers!
    let alice_client = reqwest::Client::new();
    let ask_resp: serde_json::Value = alice_client
        .post(format!("http://{}/ask", alice_d.addr))
        .bearer_auth(&alice_d.token)
        .json(&serde_json::json!({
            "target": "bob",
            "question": "What is the JWT expiry?",
            "wait_seconds": 5
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(ask_resp["status"], "answered");
    assert_eq!(ask_resp["reply"]["body"], "JWT expires in 15m");
    assert_eq!(ask_resp["reply"]["msg_type"], "answer");

    let _ = alice_d.shutdown.send(true);
    let _ = bob_d.shutdown.send(true);
}

#[tokio::test]
async fn sim_background_responder_escalates_needs_human() {
    let relay_addr = start_mock_relay().await;
    let relay_url = format!("http://{relay_addr}");

    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let alice_home = AgentLinkHome::from_path(alice_dir.path());
    let bob_home = AgentLinkHome::from_path(bob_dir.path());

    alice_home.init("alice", Some(&relay_url), Some(0)).unwrap();
    bob_home.init("bob", Some(&relay_url), Some(0)).unwrap();

    // Configure Bob's responder to escalate to human
    let mut bob_cfg = bob_home.load_config().unwrap();
    bob_cfg.responder_cmd = Some(write_mock_responder(
        &bob_dir,
        r#"{"action":"needs_human","body":"Requires Bob approval to change billing","refs":[]}"#,
    ));
    std::fs::write(
        bob_home.config_path(),
        serde_json::to_string_pretty(&bob_cfg).unwrap(),
    )
    .unwrap();

    pair_two_homes(&relay_url, &alice_home, &bob_home).await;

    let alice_d = start_daemon(&alice_home, Some(0)).await.unwrap();
    let bob_d = start_daemon(&bob_home, Some(0)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let alice_client = reqwest::Client::new();
    let ask_resp: serde_json::Value = alice_client
        .post(format!("http://{}/ask", alice_d.addr))
        .bearer_auth(&alice_d.token)
        .json(&serde_json::json!({
            "target": "bob",
            "question": "Can we switch Stripe to annual-only billing?",
            "wait_seconds": 5
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(ask_resp["status"], "answered");
    assert_eq!(ask_resp["reply"]["msg_type"], "pending_human");
    assert_eq!(ask_resp["reply"]["requires_human"], true);

    // Verify Bob's inbox marked the thread as needs_human
    let bob_inbox: Vec<StoredMessage> = reqwest::Client::new()
        .get(format!("http://{}/inbox?unread=1", bob_d.addr))
        .bearer_auth(&bob_d.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bob_inbox.len(), 1);
    assert_eq!(bob_inbox[0].status, "needs_human");

    let _ = alice_d.shutdown.send(true);
    let _ = bob_d.shutdown.send(true);
}

#[tokio::test]
async fn sim_blocked_peer_messages_are_dropped() {
    let relay_addr = start_mock_relay().await;
    let relay_url = format!("http://{relay_addr}");

    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let alice_home = AgentLinkHome::from_path(alice_dir.path());
    let bob_home = AgentLinkHome::from_path(bob_dir.path());

    let (alice_id, _, _) = alice_home.init("alice", Some(&relay_url), Some(0)).unwrap();
    bob_home.init("bob", Some(&relay_url), Some(0)).unwrap();
    pair_two_homes(&relay_url, &alice_home, &bob_home).await;

    // Bob blocks Alice
    Store::open(&bob_home.db_path())
        .unwrap()
        .block_peer(&alice_id.peer_id)
        .unwrap();

    let alice_d = start_daemon(&alice_home, Some(0)).await.unwrap();
    let bob_d = start_daemon(&bob_home, Some(0)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    reqwest::Client::new()
        .post(format!("http://{}/ask", alice_d.addr))
        .bearer_auth(&alice_d.token)
        .json(&serde_json::json!({
            "target": "bob",
            "question": "Should be dropped",
            "wait_seconds": 0
        }))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(250)).await;

    let bob_inbox: Vec<StoredMessage> = reqwest::Client::new()
        .get(format!("http://{}/inbox?unread=1", bob_d.addr))
        .bearer_auth(&bob_d.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(bob_inbox.is_empty());

    let _ = alice_d.shutdown.send(true);
    let _ = bob_d.shutdown.send(true);
}

#[tokio::test]
async fn sim_whatsapp_group_chat_across_peers_and_local() {
    let relay_addr = start_mock_relay().await;
    let relay_url = format!("http://{relay_addr}");

    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let carol_dir = TempDir::new().unwrap();
    let alice_home = AgentLinkHome::from_path(alice_dir.path());
    let bob_home = AgentLinkHome::from_path(bob_dir.path());
    let carol_home = AgentLinkHome::from_path(carol_dir.path());

    let (alice_id, _, _) = alice_home.init("alice", Some(&relay_url), Some(0)).unwrap();
    let (bob_id, _, _) = bob_home.init("bob", Some(&relay_url), Some(0)).unwrap();
    let (carol_id, _, _) = carol_home.init("carol", Some(&relay_url), Some(0)).unwrap();

    // All 3 peers know each other's public keys
    let alice_store = Store::open(&alice_home.db_path()).unwrap();
    alice_store.upsert_peer(&bob_id.to_public(), "bob", None).unwrap();
    alice_store.upsert_peer(&carol_id.to_public(), "carol", None).unwrap();

    let bob_store = Store::open(&bob_home.db_path()).unwrap();
    bob_store.upsert_peer(&alice_id.to_public(), "alice", None).unwrap();
    bob_store.upsert_peer(&carol_id.to_public(), "carol", None).unwrap();

    let carol_store = Store::open(&carol_home.db_path()).unwrap();
    carol_store.upsert_peer(&alice_id.to_public(), "alice", None).unwrap();
    carol_store.upsert_peer(&bob_id.to_public(), "bob", None).unwrap();

    let alice_d = start_daemon(&alice_home, Some(0)).await.unwrap();
    let bob_d = start_daemon(&bob_home, Some(0)).await.unwrap();
    let carol_d = start_daemon(&carol_home, Some(0)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let client = reqwest::Client::new();

    // Register local:frontend on Alice's laptop
    client
        .post(format!("http://{}/sessions/register", alice_d.addr))
        .bearer_auth(&alice_d.token)
        .json(&serde_json::json!({
            "name": "frontend",
            "agent_kind": "codex",
            "project_root": "/repo/frontend"
        }))
        .send()
        .await
        .unwrap();

    // Alice creates group "team" mixing remote peers (bob, carol) + local:frontend
    let grp_resp: serde_json::Value = client
        .post(format!("http://{}/groups", alice_d.addr))
        .bearer_auth(&alice_d.token)
        .json(&serde_json::json!({
            "name": "team",
            "members": ["bob", "carol", "local:frontend"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(grp_resp["name"], "team");

    // Alice asks group:team
    let ask_resp: serde_json::Value = client
        .post(format!("http://{}/ask", alice_d.addr))
        .bearer_auth(&alice_d.token)
        .json(&serde_json::json!({
            "target": "group:team",
            "question": "API v2 is ready — any blockers?",
            "wait_seconds": 0
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let thread_id = ask_resp["thread_id"].as_str().unwrap().to_string();

    tokio::time::sleep(Duration::from_millis(250)).await;

    // Verify local:frontend, Bob, and Carol all received the group message
    let local_msgs: Vec<StoredMessage> = client
        .get(format!("http://{}/inbox?unread=1&session=frontend", alice_d.addr))
        .bearer_auth(&alice_d.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(local_msgs.len(), 1);
    assert_eq!(local_msgs[0].body, "API v2 is ready — any blockers?");

    let bob_msgs: Vec<StoredMessage> = client
        .get(format!("http://{}/inbox?unread=1", bob_d.addr))
        .bearer_auth(&bob_d.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bob_msgs.len(), 1);

    let carol_msgs: Vec<StoredMessage> = client
        .get(format!("http://{}/inbox?unread=1", carol_d.addr))
        .bearer_auth(&carol_d.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(carol_msgs.len(), 1);

    // Bob replies to the group thread -> both Alice and Carol receive it!
    client
        .post(format!("http://{}/reply", bob_d.addr))
        .bearer_auth(&bob_d.token)
        .json(&serde_json::json!({
            "thread_id": thread_id,
            "body": "No blockers from Bob!"
        }))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(250)).await;

    let alice_replies: Vec<StoredMessage> = client
        .get(format!("http://{}/inbox?unread=1&thread_id={thread_id}", alice_d.addr))
        .bearer_auth(&alice_d.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(alice_replies.iter().any(|m| m.body == "No blockers from Bob!"));

    let carol_thread: Vec<StoredMessage> = client
        .get(format!("http://{}/inbox?unread=1&thread_id={thread_id}", carol_d.addr))
        .bearer_auth(&carol_d.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(carol_thread.iter().any(|m| m.body == "No blockers from Bob!"));

    let _ = alice_d.shutdown.send(true);
    let _ = bob_d.shutdown.send(true);
    let _ = carol_d.shutdown.send(true);
}

