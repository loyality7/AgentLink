use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use agentlink_core::{
    crypto::EncryptedEnvelope,
    identity::PublicIdentity,
    protocol::{AgentMessage, MessageType},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::Result;

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPeer {
    pub peer_id: String,
    pub name: String,
    pub sign_pk: String,
    pub kx_pk: String,
    pub project: Option<String>,
    pub created_at: i64,
}

impl StoredPeer {
    pub fn to_public_identity(&self) -> PublicIdentity {
        PublicIdentity {
            peer_id: self.peer_id.clone(),
            display_name: self.name.clone(),
            sign_pk: self.sign_pk.clone(),
            kx_pk: self.kx_pk.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredGroup {
    pub group_id: String,
    pub name: String,
    pub members: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub thread_id: String,
    pub reply_to: Option<String>,
    pub from_peer: String,
    pub to_peer: String,
    pub direction: String,
    pub msg_type: MessageType,
    pub project: Option<String>,
    pub hop: u8,
    pub max_hops: u8,
    pub requires_human: bool,
    pub body: String,
    pub refs: Vec<String>,
    pub group_id: Option<String>,
    pub status: String,
    pub created_at: i64,
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS peers (
                peer_id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                sign_pk TEXT NOT NULL,
                kx_pk TEXT NOT NULL,
                project TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                reply_to TEXT,
                from_peer TEXT NOT NULL,
                to_peer TEXT NOT NULL,
                direction TEXT NOT NULL,
                msg_type TEXT NOT NULL,
                project TEXT,
                hop INTEGER NOT NULL,
                max_hops INTEGER NOT NULL,
                requires_human INTEGER NOT NULL,
                body TEXT NOT NULL,
                refs_json TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(thread_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_messages_status ON messages(status, created_at);
            CREATE TABLE IF NOT EXISTS outbox (
                id TEXT PRIMARY KEY,
                to_peer TEXT NOT NULL,
                envelope_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS blocked_peers (
                peer_id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS groups (
                group_id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                members_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS pending_pairs (
                code TEXT PRIMARY KEY,
                peer_name TEXT NOT NULL,
                expires_at INTEGER NOT NULL
            );
            ",
        )?;
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN group_id TEXT", []);
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn upsert_peer(&self, peer: &PublicIdentity, local_name: &str, project: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO peers (peer_id, name, sign_pk, kx_pk, project, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(peer_id) DO UPDATE SET
               name = excluded.name,
               sign_pk = excluded.sign_pk,
               kx_pk = excluded.kx_pk,
               project = COALESCE(excluded.project, peers.project)",
            params![
                peer.peer_id,
                local_name,
                peer.sign_pk,
                peer.kx_pk,
                project,
                now_unix()
            ],
        )?;
        Ok(())
    }

    pub fn get_peer(&self, name_or_id: &str) -> Result<Option<StoredPeer>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT peer_id, name, sign_pk, kx_pk, project, created_at
             FROM peers WHERE name = ?1 OR peer_id = ?1 LIMIT 1",
            params![name_or_id],
            |row| {
                Ok(StoredPeer {
                    peer_id: row.get(0)?,
                    name: row.get(1)?,
                    sign_pk: row.get(2)?,
                    kx_pk: row.get(3)?,
                    project: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_peers(&self) -> Result<Vec<StoredPeer>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT peer_id, name, sign_pk, kx_pk, project, created_at FROM peers ORDER BY name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredPeer {
                peer_id: row.get(0)?,
                name: row.get(1)?,
                sign_pk: row.get(2)?,
                kx_pk: row.get(3)?,
                project: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn insert_message(
        &self,
        msg: &AgentMessage,
        from_peer: &str,
        to_peer: &str,
        direction: &str,
        status: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let msg_type_str = serde_json::to_string(&msg.msg_type)?;
        let refs_json = serde_json::to_string(&msg.refs)?;
        let group_id = msg.group.as_ref().map(|g| g.group_id.as_str());
        conn.execute(
            "INSERT OR IGNORE INTO messages (
                id, thread_id, reply_to, from_peer, to_peer, direction,
                msg_type, project, hop, max_hops, requires_human, body,
                refs_json, group_id, status, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                msg.id,
                msg.thread_id,
                msg.reply_to,
                from_peer,
                to_peer,
                direction,
                msg_type_str,
                msg.project,
                msg.hop as i64,
                msg.max_hops as i64,
                msg.requires_human as i64,
                msg.body,
                refs_json,
                group_id,
                status,
                now_unix(),
            ],
        )?;
        Ok(())
    }

    pub fn list_messages(
        &self,
        thread_id: Option<&str>,
        unread_only: bool,
        to_peer_filter: Option<&str>,
    ) -> Result<Vec<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, thread_id, reply_to, from_peer, to_peer, direction,
                    msg_type, project, hop, max_hops, requires_human, body,
                    refs_json, group_id, status, created_at
             FROM messages WHERE 1=1",
        );
        let mut bind_vals: Vec<String> = Vec::new();
        if let Some(tid) = thread_id {
            bind_vals.push(tid.to_string());
            sql.push_str(&format!(" AND thread_id = ?{}", bind_vals.len()));
        }
        if unread_only {
            sql.push_str(" AND status IN ('unread', 'needs_human')");
        }
        if let Some(to) = to_peer_filter {
            bind_vals.push(to.to_string());
            sql.push_str(&format!(" AND to_peer = ?{}", bind_vals.len()));
        }
        sql.push_str(" ORDER BY created_at ASC");

        let mut stmt = conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> =
            bind_vals.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params_refs.as_slice(), row_to_stored_message)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn latest_incoming_in_thread(&self, thread_id: &str) -> Result<Option<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, thread_id, reply_to, from_peer, to_peer, direction,
                    msg_type, project, hop, max_hops, requires_human, body,
                    refs_json, group_id, status, created_at
             FROM messages
             WHERE thread_id = ?1 AND direction = 'in'
             ORDER BY rowid DESC LIMIT 1",
            params![thread_id],
            row_to_stored_message,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn latest_reply_in_thread(&self, thread_id: &str, after_msg_id: &str) -> Result<Option<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, thread_id, reply_to, from_peer, to_peer, direction,
                    msg_type, project, hop, max_hops, requires_human, body,
                    refs_json, group_id, status, created_at
             FROM messages
             WHERE thread_id = ?1 AND direction = 'in' AND id != ?2
             ORDER BY rowid DESC LIMIT 1",
            params![thread_id, after_msg_id],
            row_to_stored_message,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn mark_read(&self, msg_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET status = 'read' WHERE id = ?1",
            params![msg_id],
        )?;
        Ok(())
    }

    pub fn mark_thread_needs_human(&self, thread_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET status = 'needs_human', requires_human = 1 WHERE thread_id = ?1",
            params![thread_id],
        )?;
        Ok(())
    }

    pub fn enqueue_outbox(&self, env: &EncryptedEnvelope) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let json = serde_json::to_string(env)?;
        conn.execute(
            "INSERT OR REPLACE INTO outbox (id, to_peer, envelope_json, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![env.id, env.to, json, now_unix()],
        )?;
        Ok(())
    }

    pub fn list_outbox(&self) -> Result<Vec<EncryptedEnvelope>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT envelope_json FROM outbox ORDER BY created_at ASC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    pub fn delete_outbox(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM outbox WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn block_peer(&self, peer_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO blocked_peers (peer_id, created_at) VALUES (?1, ?2)",
            params![peer_id, now_unix()],
        )?;
        Ok(())
    }

    pub fn unblock_peer(&self, peer_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM blocked_peers WHERE peer_id = ?1",
            params![peer_id],
        )?;
        Ok(())
    }

    pub fn is_blocked(&self, peer_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let found: Option<String> = conn
            .query_row(
                "SELECT peer_id FROM blocked_peers WHERE peer_id = ?1 LIMIT 1",
                params![peer_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn upsert_group(&self, group_id: &str, name: &str, members: &[String]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let members_json = serde_json::to_string(members)?;
        conn.execute(
            "INSERT INTO groups (group_id, name, members_json, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(group_id) DO UPDATE SET
               members_json = excluded.members_json",
            params![group_id, name, members_json, now_unix()],
        )?;
        Ok(())
    }

    pub fn get_group(&self, name_or_id: &str) -> Result<Option<StoredGroup>> {
        let lookup = name_or_id.strip_prefix("group:").unwrap_or(name_or_id);
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT group_id, name, members_json, created_at
             FROM groups WHERE name = ?1 OR group_id = ?1 LIMIT 1",
            params![lookup],
            |row| {
                let members_json: String = row.get(2)?;
                let members: Vec<String> = serde_json::from_str(&members_json).unwrap_or_default();
                Ok(StoredGroup {
                    group_id: row.get(0)?,
                    name: row.get(1)?,
                    members,
                    created_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_groups(&self) -> Result<Vec<StoredGroup>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT group_id, name, members_json, created_at FROM groups ORDER BY name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let members_json: String = row.get(2)?;
            let members: Vec<String> = serde_json::from_str(&members_json).unwrap_or_default();
            Ok(StoredGroup {
                group_id: row.get(0)?,
                name: row.get(1)?,
                members,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn save_pending_pair(&self, code: &str, peer_name: &str, expires_at: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO pending_pairs (code, peer_name, expires_at) VALUES (?1, ?2, ?3)",
            params![code, peer_name, expires_at],
        )?;
        Ok(())
    }

    pub fn list_pending_pairs(&self) -> Result<Vec<(String, String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix();
        conn.execute("DELETE FROM pending_pairs WHERE expires_at < ?1", params![now])?;
        let mut stmt = conn.prepare("SELECT code, peer_name, expires_at FROM pending_pairs")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn delete_pending_pair(&self, code: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM pending_pairs WHERE code = ?1", params![code])?;
        Ok(())
    }
}

fn row_to_stored_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMessage> {
    let msg_type_str: String = row.get(6)?;
    let refs_json: String = row.get(12)?;
    let msg_type: MessageType =
        serde_json::from_str(&msg_type_str).unwrap_or(MessageType::Question);
    let refs: Vec<String> = serde_json::from_str(&refs_json).unwrap_or_default();

    Ok(StoredMessage {
        id: row.get(0)?,
        thread_id: row.get(1)?,
        reply_to: row.get(2)?,
        from_peer: row.get(3)?,
        to_peer: row.get(4)?,
        direction: row.get(5)?,
        msg_type,
        project: row.get(7)?,
        hop: row.get::<_, i64>(8)? as u8,
        max_hops: row.get::<_, i64>(9)? as u8,
        requires_human: row.get::<_, i64>(10)? != 0,
        body: row.get(11)?,
        refs,
        group_id: row.get(13)?,
        status: row.get(14)?,
        created_at: row.get(15)?,
    })
}

