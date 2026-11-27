use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use agentlink_core::{
    crypto::{decrypt_from_peer, EncryptedEnvelope},
    identity::Identity,
    protocol::{AgentMessage, PairingBundle, RelayFrame},
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use crate::{
    config::DaemonConfig,
    error::{DaemonError, Result},
    responder::maybe_auto_respond,
    store::Store,
};

fn now_unix_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn to_ws_url(relay_url: &str, peer_id: &str) -> String {
    let base = relay_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws_base}/ws?peer_id={peer_id}")
}

/// Persistent WebSocket loop connecting `agentlinkd` to `agentlink-relay`.
#[allow(clippy::too_many_arguments)]
pub async fn run_relay_loop(
    relay_url: String,
    identity: Identity,
    config: DaemonConfig,
    store: Arc<Store>,
    tx_out: mpsc::Sender<EncryptedEnvelope>,
    mut rx_out: mpsc::Receiver<EncryptedEnvelope>,
    notify_thread: broadcast::Sender<String>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut backoff = Duration::from_millis(200);

    loop {
        if *shutdown.borrow() {
            break;
        }

        let ws_url = to_ws_url(&relay_url, &identity.peer_id);
        debug!("connecting to relay: {ws_url}");

        match connect_async(&ws_url).await {
            Ok((ws_stream, _)) => {
                info!("connected to relay");
                backoff = Duration::from_millis(200);

                let (mut write, mut read) = ws_stream.split();

                // Authenticate WebSocket session
                let ts = now_unix_u64();
                let canonical = format!("auth:{}:{}", identity.peer_id, ts);
                let sig = match identity.sign(canonical.as_bytes()) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("failed to sign auth frame: {e}");
                        continue;
                    }
                };
                let auth_frame = RelayFrame::Auth {
                    peer_id: identity.peer_id.clone(),
                    timestamp: ts,
                    sign_pk: identity.sign_pk.clone(),
                    signature: sig,
                };
                if let Ok(json) = serde_json::to_string(&auth_frame) {
                    if write.send(Message::Text(json.into())).await.is_err() {
                        continue;
                    }
                }

                // Wait for auth_ok before flushing outbox
                let mut authed = false;
                while let Some(frame_res) = read.next().await {
                    match frame_res {
                        Ok(Message::Text(txt)) => {
                            if let Ok(RelayFrame::AuthOk { .. }) = serde_json::from_str(&txt) {
                                authed = true;
                                break;
                            } else if let Ok(RelayFrame::Error { message }) =
                                serde_json::from_str(&txt)
                            {
                                warn!("relay auth error: {message}");
                                break;
                            }
                        }
                        Ok(Message::Close(_)) | Err(_) => break,
                        _ => {}
                    }
                }
                if !authed {
                    tokio::time::sleep(backoff).await;
                    continue;
                }

                // Flush any queued outbox envelopes
                if let Ok(pending) = store.list_outbox() {
                    for env in pending {
                        let frame = RelayFrame::Send {
                            envelope: env.clone(),
                        };
                        if let Ok(json) = serde_json::to_string(&frame) {
                            if write.send(Message::Text(json.into())).await.is_ok() {
                                let _ = store.delete_outbox(&env.id);
                            } else {
                                break;
                            }
                        }
                    }
                }

                // Active bidirectional loop + periodic outbox retry & pending_pairs check
                let mut retry_tick = tokio::time::interval(Duration::from_secs(5));
                retry_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                loop {
                    tokio::select! {
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                let _ = write.close().await;
                                return;
                            }
                        }
                        _ = retry_tick.tick() => {
                            if let Ok(pending) = store.list_outbox() {
                                for env in pending {
                                    let frame = RelayFrame::Send { envelope: env.clone() };
                                    if let Ok(json) = serde_json::to_string(&frame) {
                                        if write.send(Message::Text(json.into())).await.is_ok() {
                                            let _ = store.delete_outbox(&env.id);
                                        } else {
                                            break;
                                        }
                                    }
                                }
                            }
                            if let Ok(pairs) = store.list_pending_pairs() {
                                for (code, peer_name, _) in pairs {
                                    let ack_code = format!("{code}:ack");
                                    if let Ok(Some(bundle)) = fetch_pairing(&relay_url, &ack_code).await {
                                        let name = if peer_name == "peer" {
                                            bundle.peer.display_name.clone()
                                        } else {
                                            peer_name.clone()
                                        };
                                        if store.upsert_peer(&bundle.peer, &name, bundle.project.as_deref()).is_ok() {
                                            let _ = store.delete_pending_pair(&code);
                                            crate::responder::notify_human(
                                                "AgentLink: Peer Paired",
                                                &format!("Connected with {name}"),
                                            );
                                            info!("background pair completed with '{name}' ({})", bundle.peer.peer_id);
                                        }
                                    }
                                }
                            }
                        }
                        maybe_env = rx_out.recv() => {
                            let Some(env) = maybe_env else { return; };
                            let frame = RelayFrame::Send { envelope: env.clone() };
                            if let Ok(json) = serde_json::to_string(&frame) {
                                if write.send(Message::Text(json.into())).await.is_ok() {
                                    let _ = store.delete_outbox(&env.id);
                                } else {
                                    break;
                                }
                            }
                        }
                        maybe_msg = read.next() => {
                            match maybe_msg {
                                Some(Ok(Message::Text(txt))) => {
                                    if let Ok(frame) = serde_json::from_str::<RelayFrame>(&txt) {
                                        match frame {
                                            RelayFrame::Deliver { envelope } => {
                                                match handle_delivered(&identity, &store, &envelope, &notify_thread) {
                                                    Ok((sender_name, msg)) => {
                                                        tokio::spawn(maybe_auto_respond(
                                                            identity.clone(),
                                                            config.clone(),
                                                            Arc::clone(&store),
                                                            tx_out.clone(),
                                                            notify_thread.clone(),
                                                            sender_name,
                                                            msg,
                                                        ));
                                                    }
                                                    Err(e) => {
                                                        warn!("failed to process delivered envelope {}: {e}", envelope.id);
                                                    }
                                                }
                                                let ack = RelayFrame::Ack { msg_id: envelope.id };
                                                if let Ok(ack_json) = serde_json::to_string(&ack) {
                                                    let _ = write.send(Message::Text(ack_json.into())).await;
                                                }
                                            }
                                            RelayFrame::Error { message } => {
                                                warn!("relay error: {message}");
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                                _ => {}
                            }
                        }
                    }
                }
            }
            Err(e) => {
                debug!("relay connect error: {e}");
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

fn handle_delivered(
    identity: &Identity,
    store: &Store,
    envelope: &EncryptedEnvelope,
    notify_thread: &broadcast::Sender<String>,
) -> Result<(String, AgentMessage)> {
    if store.is_blocked(&envelope.from)? {
        return Err(DaemonError::Other(format!(
            "dropped message from blocked peer {}",
            envelope.from
        )));
    }
    let sender = store
        .get_peer(&envelope.from)?
        .ok_or_else(|| DaemonError::PeerNotFound(envelope.from.clone()))?;

    let plaintext = decrypt_from_peer(identity, &sender.to_public_identity(), envelope)?;
    let msg: AgentMessage = serde_json::from_slice(&plaintext)?;

    if let Some(ref g) = msg.group {
        let _ = store.upsert_group(&g.group_id, &g.name, &g.members);
    }

    let status = if msg.requires_human {
        "needs_human"
    } else {
        "unread"
    };
    store.insert_message(&msg, &sender.name, "self", "in", status)?;
    let _ = notify_thread.send(msg.thread_id.clone());
    Ok((sender.name, msg))
}

/// Publish a pairing bundle to the relay's `/pair` endpoint.
pub async fn publish_pairing(relay_url: &str, bundle: &PairingBundle) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/pair", relay_url.trim_end_matches('/'));
    let payload = serde_json::to_string(bundle)?;
    let resp = client
        .post(url)
        .json(&serde_json::json!({
            "code": bundle.code,
            "payload": payload,
            "expires_at": bundle.expires_at,
        }))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(DaemonError::Other(format!(
            "relay /pair returned {}",
            resp.status()
        )));
    }
    Ok(())
}

/// Fetch a pairing bundle from the relay's `/pair/:code` endpoint.
pub async fn fetch_pairing(relay_url: &str, code: &str) -> Result<Option<PairingBundle>> {
    let client = reqwest::Client::new();
    let url = format!("{}/pair/{}", relay_url.trim_end_matches('/'), code);
    let resp = client.get(url).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(DaemonError::Other(format!(
            "relay /pair/{code} returned {}",
            resp.status()
        )));
    }
    let bundle: PairingBundle = resp.json().await?;
    bundle.peer.verify_peer_id()?;
    Ok(Some(bundle))
}

