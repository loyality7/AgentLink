use rand::{rngs::OsRng, Rng};
use serde::{Deserialize, Serialize};

use crate::{crypto::EncryptedEnvelope, identity::PublicIdentity};

/// Semantic type of a peer-to-peer agent message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Question,
    Answer,
    ClarificationNeeded,
    PendingHuman,
    Error,
}

/// Group metadata carried inside encrypted group messages (hidden from the relay).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupMeta {
    pub group_id: String,
    pub name: String,
    /// Member identifiers (peer_ids for remote peers, or `local:<session>`).
    pub members: Vec<String>,
}

/// Structured plaintext payload inside an `EncryptedEnvelope`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMessage {
    pub v: u8,
    pub id: String,
    pub thread_id: String,
    pub reply_to: Option<String>,
    #[serde(rename = "type")]
    pub msg_type: MessageType,
    pub project: Option<String>,
    pub hop: u8,
    pub max_hops: u8,
    pub requires_human: bool,
    pub body: String,
    #[serde(default)]
    pub refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupMeta>,
}

impl AgentMessage {
    pub fn new_question(
        id: impl Into<String>,
        thread_id: impl Into<String>,
        project: Option<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            v: 1,
            id: id.into(),
            thread_id: thread_id.into(),
            reply_to: None,
            msg_type: MessageType::Question,
            project,
            hop: 1,
            max_hops: 5,
            requires_human: false,
            body: body.into(),
            refs: Vec::new(),
            group: None,
        }
    }

    /// Returns true if an automated responder is allowed to reply without hitting the hop cap.
    pub fn can_auto_reply(&self) -> bool {
        !self.requires_human && self.hop < self.max_hops
    }
}

/// Pairing record published briefly on the relay during `agentlink pair` / `agentlink join`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingBundle {
    pub v: u8,
    pub code: String,
    pub peer: PublicIdentity,
    pub project: Option<String>,
    pub expires_at: u64,
}

/// Generate a human-friendly short pairing code like `AL-7K9M-2P4X`.
pub fn generate_pairing_code() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut rng = OsRng;
    let mut part = |len: usize| -> String {
        (0..len)
            .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
            .collect()
    };
    format!("AL-{}-{}", part(4), part(4))
}

/// WebSocket frames exchanged between `agentlink daemon` and `agentlink-relay`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RelayFrame {
    /// Client authenticates its WebSocket session for `peer_id`.
    Auth {
        peer_id: String,
        timestamp: u64,
        sign_pk: String,
        signature: String,
    },
    /// Relay confirms authentication.
    AuthOk { peer_id: String },
    /// Client sends an encrypted envelope to `envelope.to`.
    Send { envelope: EncryptedEnvelope },
    /// Relay delivers an encrypted envelope to the recipient.
    Deliver { envelope: EncryptedEnvelope },
    /// Recipient acknowledges delivery so the relay can delete the queued copy.
    Ack { msg_id: String },
    /// Relay error notification.
    Error { message: String },
}

