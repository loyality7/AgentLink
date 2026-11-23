use std::path::PathBuf;

use agentlink_daemon::config::AgentLinkHome;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

struct McpClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
    session_name: String,
}

impl McpClient {
    fn from_home(home: &AgentLinkHome) -> Result<Self, Box<dyn std::error::Error>> {
        let cfg = home.load_config()?;
        let token = home.load_token()?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let session_name = std::env::var("AGENTLINK_SESSION").unwrap_or_else(|_| {
            cwd.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("session")
                .to_string()
        });
        Ok(Self {
            base_url: format!("http://127.0.0.1:{}", cfg.listen_port),
            token,
            http: reqwest::Client::new(),
            session_name,
        })
    }

    async fn register_session(&self) {
        let cwd = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .to_string_lossy()
            .to_string();
        let agent_kind =
            std::env::var("AGENTLINK_AGENT").unwrap_or_else(|_| "mcp-client".to_string());
        let _ = self
            .http
            .post(format!("{}/sessions/register", self.base_url))
            .bearer_auth(&self.token)
            .json(&json!({
                "name": self.session_name,
                "agent_kind": agent_kind,
                "project_root": cwd,
            }))
            .send()
            .await;
    }

    async fn call_tool(&self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "list_peers" => {
                let resp = self
                    .http
                    .get(format!("{}/peers", self.base_url))
                    .bearer_auth(&self.token)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                resp.json::<Value>().await.map_err(|e| e.to_string())
            }
            "ask_peer" => {
                let target = args
                    .get("target")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'target'")?;
                let question = args
                    .get("question")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'question'")?;
                let wait_seconds = args.get("wait_seconds").and_then(|v| v.as_u64()).unwrap_or(20);
                let project = args.get("project").and_then(|v| v.as_str());
                let thread_id = args.get("thread_id").and_then(|v| v.as_str());

                let resp = self
                    .http
                    .post(format!("{}/ask", self.base_url))
                    .bearer_auth(&self.token)
                    .json(&json!({
                        "target": target,
                        "question": question,
                        "wait_seconds": wait_seconds,
                        "project": project,
                        "thread_id": thread_id,
                        "from_session": self.session_name,
                    }))
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                resp.json::<Value>().await.map_err(|e| e.to_string())
            }
            "reply_peer" => {
                let thread_id = args
                    .get("thread_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'thread_id'")?;
                let body = args
                    .get("body")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'body'")?;
                let requires_human = args
                    .get("requires_human")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let refs = args.get("refs").cloned().unwrap_or_else(|| json!([]));

                let resp = self
                    .http
                    .post(format!("{}/reply", self.base_url))
                    .bearer_auth(&self.token)
                    .json(&json!({
                        "thread_id": thread_id,
                        "body": body,
                        "requires_human": requires_human,
                        "refs": refs,
                        "from_session": self.session_name,
                    }))
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                resp.json::<Value>().await.map_err(|e| e.to_string())
            }
            "check_inbox" => {
                let unread = args
                    .get("unread_only")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let mut url = format!(
                    "{}/inbox?unread={}",
                    self.base_url,
                    if unread { 1 } else { 0 }
                );
                if let Some(tid) = args.get("thread_id").and_then(|v| v.as_str()) {
                    url.push_str(&format!("&thread_id={tid}"));
                }
                let resp = self
                    .http
                    .get(url)
                    .bearer_auth(&self.token)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                resp.json::<Value>().await.map_err(|e| e.to_string())
            }
            "escalate_to_user" => {
                let thread_id = args
                    .get("thread_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'thread_id'")?;
                let reason = args
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'reason'")?;
                let resp = self
                    .http
                    .post(format!("{}/escalate", self.base_url))
                    .bearer_auth(&self.token)
                    .json(&json!({
                        "thread_id": thread_id,
                        "reason": reason,
                    }))
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                resp.json::<Value>().await.map_err(|e| e.to_string())
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }
}

fn tool_definitions() -> Value {
    json!({
        "tools": [
            {
                "name": "list_peers",
                "description": "List paired remote peers and active local agent sessions on this machine.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "ask_peer",
                "description": "Ask a remote peer or local agent session (e.g. 'bob' or 'local:frontend') a question about the project and wait for a reply.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "Peer name (e.g. 'bob') or local session ('local:frontend')." },
                        "question": { "type": "string", "description": "Clear, specific question for the peer's agent." },
                        "wait_seconds": { "type": "integer", "description": "Seconds to wait for an inline reply (default 20)." },
                        "project": { "type": "string" },
                        "thread_id": { "type": "string", "description": "Optional existing thread_id for follow-up questions." }
                    },
                    "required": ["target", "question"]
                }
            },
            {
                "name": "reply_peer",
                "description": "Reply to an incoming peer question in a thread.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "thread_id": { "type": "string" },
                        "body": { "type": "string" },
                        "requires_human": { "type": "boolean" },
                        "refs": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["thread_id", "body"]
                }
            },
            {
                "name": "check_inbox",
                "description": "Check incoming messages or pending replies from peers.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "thread_id": { "type": "string" },
                        "unread_only": { "type": "boolean" }
                    }
                }
            },
            {
                "name": "escalate_to_user",
                "description": "Flag a thread as needing explicit human confirmation or decision.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "thread_id": { "type": "string" },
                        "reason": { "type": "string" }
                    },
                    "required": ["thread_id", "reason"]
                }
            }
        ]
    })
}

/// Process a single JSON-RPC request line and return an optional response.
pub async fn handle_rpc_line(
    line: &str,
    home: &AgentLinkHome,
) -> Option<String> {
    let req: JsonRpcRequest = serde_json::from_str(line).ok()?;
    let id = req.id?;

    let resp = match req.method.as_str() {
        "initialize" => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "agentlink", "version": env!("CARGO_PKG_VERSION") }
            })),
            error: None,
        },
        "tools/list" => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(tool_definitions()),
            error: None,
        },
        "tools/call" => {
            let tool_name = req
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args = req.params.get("arguments").cloned().unwrap_or_else(|| json!({}));

            match McpClient::from_home(home) {
                Ok(client) => match client.call_tool(tool_name, &args).await {
                    Ok(val) => JsonRpcResponse {
                        jsonrpc: "2.0",
                        id,
                        result: Some(json!({
                            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&val).unwrap_or_default() }]
                        })),
                        error: None,
                    },
                    Err(err) => JsonRpcResponse {
                        jsonrpc: "2.0",
                        id,
                        result: Some(json!({
                            "isError": true,
                            "content": [{ "type": "text", "text": err }]
                        })),
                        error: None,
                    },
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: Some(json!({
                        "isError": true,
                        "content": [{ "type": "text", "text": err.to_string() }]
                    })),
                    error: None,
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("method not found: {}", req.method),
            }),
        },
    };

    serde_json::to_string(&resp).ok()
}

/// Run the stdio MCP server loop until EOF.
pub async fn run_mcp_stdio(home: &AgentLinkHome) -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(client) = McpClient::from_home(home) {
        client.register_session().await;
    }

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();

    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(out) = handle_rpc_line(trimmed, home).await {
            stdout.write_all(out.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
