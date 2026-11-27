mod setup;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentlink_core::protocol::{generate_pairing_code, PairingBundle};
use agentlink_daemon::{
    config::AgentLinkHome,
    ensure_daemon_running,
    relay_client::{fetch_pairing, publish_pairing},
    start_daemon,
    store::Store,
};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agentlink", version, about = "Lightweight E2EE agent-to-agent bridge")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize ~/.agentlink/ identity and config
    Init {
        #[arg(long)]
        name: String,
        #[arg(long)]
        relay: Option<String>,
        #[arg(long)]
        port: Option<u16>,
    },
    /// Manage the background daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Generate a one-time pairing code for a friend
    Pair {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = "peer")]
        name: String,
        #[arg(long)]
        no_wait: bool,
    },
    /// Join a friend using their pairing code
    Join {
        code: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        project: Option<String>,
    },
    /// List paired remote peers and active local agent sessions
    Peers,
    /// Ask a peer or local agent session a question
    Ask {
        target: String,
        question: String,
        #[arg(long, default_value_t = 15)]
        wait: u64,
        #[arg(long)]
        project: Option<String>,
    },
    /// Reply to a thread
    Reply {
        thread_id: String,
        body: String,
        #[arg(long)]
        requires_human: bool,
    },
    /// Inspect inbox messages
    Inbox {
        #[arg(long)]
        unread: bool,
        #[arg(long)]
        thread: Option<String>,
    },
    /// Fast hook check for live editor sessions (prints JSON if unread messages exist)
    HookCheck {
        #[arg(long)]
        session: Option<String>,
        /// Output format: json (default) or codex-stop (acks messages before returning)
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Run the stdio MCP server (for Claude, Codex, Antigravity)
    Mcp,
    /// Auto-register AgentLink MCP in Claude Code, Claude Desktop, Codex, and Antigravity
    Setup {
        #[arg(long)]
        dry_run: bool,
    },
    /// Block a peer so their messages are dropped before decryption
    Block {
        peer: String,
    },
    /// Unblock a previously blocked peer
    Unblock {
        peer: String,
    },
    /// Manage WhatsApp-style agent groups (can mix remote friends and local sessions)
    Group {
        #[command(subcommand)]
        action: GroupAction,
    },
}

#[derive(Subcommand)]
enum GroupAction {
    /// Create a group with comma-separated members (e.g. bob,carol,local:frontend)
    Create {
        name: String,
        #[arg(long, value_delimiter = ',')]
        members: Vec<String>,
    },
    /// List groups
    List,
    /// Add a member to a group
    Add {
        group: String,
        member: String,
    },
    /// Remove a member from a group
    Remove {
        group: String,
        member: String,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Run the daemon (pass --background to detach)
    Start {
        #[arg(long)]
        background: bool,
    },
    /// Stop a background daemon using daemon.pid
    Stop,
    /// Check daemon health
    Status,
}

fn now_unix_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agentlink=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let home = AgentLinkHome::resolve()?;

    match cli.command {
        Commands::Init { name, relay, port } => {
            let (id, cfg, _) = home.init(&name, relay.as_deref(), port)?;
            let _ = Store::open(&home.db_path())?;
            println!("Initialized AgentLink");
            println!("  Name:    {}", id.display_name);
            println!("  Peer ID: {}", id.peer_id);
            println!("  Relay:   {}", cfg.relay_url);
            println!("  Port:    {}", cfg.listen_port);
        }
        Commands::Daemon { action } => match action {
            DaemonAction::Start { background } => {
                if background {
                    let exe = std::env::current_exe()?;
                    let mut cmd = std::process::Command::new(exe);
                    cmd.args(["daemon", "start"])
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());
                    #[cfg(target_os = "windows")]
                    {
                        use std::os::windows::process::CommandExt;
                        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                        const DETACHED_PROCESS: u32 = 0x0000_0008;
                        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
                    }
                    let child = cmd.spawn()?;
                    let cfg = home.load_config()?;
                    let token = home.load_token()?;
                    let url = format!("http://127.0.0.1:{}/health", cfg.listen_port);
                    let client = reqwest::Client::new();
                    let mut ready = false;
                    for _ in 0..30 {
                        if let Ok(r) = client.get(&url).bearer_auth(&token).send().await {
                            if r.status().is_success() {
                                ready = true;
                                break;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    if ready {
                        println!("agentlink daemon started in background (pid {})", child.id());
                    } else {
                        return Err("daemon spawned but failed to respond on localhost".into());
                    }
                } else {
                    let handle = start_daemon(&home, None).await?;
                    println!("agentlink daemon listening on http://{}", handle.addr);
                    tokio::signal::ctrl_c().await?;
                    let _ = handle.shutdown.send(true);
                }
            }
            DaemonAction::Stop => {
                let pid_path = home.pid_path();
                if !pid_path.exists() {
                    println!("No daemon.pid found.");
                    return Ok(());
                }
                let pid_str = std::fs::read_to_string(&pid_path)?;
                let pid = pid_str.trim();
                #[cfg(target_os = "windows")]
                {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", pid, "/F"])
                        .status();
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = std::process::Command::new("kill")
                        .args(["-TERM", pid])
                        .status();
                }
                let _ = std::fs::remove_file(&pid_path);
                println!("Stopped daemon (pid {pid}).");
            }
            DaemonAction::Status => {
                let cfg = home.load_config()?;
                let token = home.load_token()?;
                let url = format!("http://127.0.0.1:{}/health", cfg.listen_port);
                let client = reqwest::Client::new();
                let resp = client.get(url).bearer_auth(token).send().await?;
                println!("{}", resp.text().await?);
            }
        },
        Commands::Pair {
            project,
            name,
            no_wait,
        } => {
            let _ = ensure_daemon_running(&home).await;
            let id = home.load_identity()?;
            let cfg = home.load_config()?;
            let store = Store::open(&home.db_path())?;
            let code = generate_pairing_code();
            let expires_at = now_unix_u64() + 600;
            let bundle = PairingBundle {
                v: 1,
                code: code.clone(),
                peer: id.to_public(),
                project,
                expires_at,
            };
            publish_pairing(&cfg.relay_url, &bundle).await?;
            store.save_pending_pair(&code, &name, expires_at as i64)?;
            println!("Copy and send the following message to your friend (they can paste it directly into their agent):\n");
            println!(
                "Hey agent — please connect to {} on AgentLink. Make sure `agentlink` is installed and initialized with relay `{}`, then run:\n  agentlink join {} --name {} --project .\nOnce done, let me know we're paired!",
                id.display_name, cfg.relay_url, code, id.display_name
            );
            println!();
            if !no_wait {
                println!("Your background daemon is watching for your friend to join (you can close this terminal).");
            }
        }
        Commands::Join {
            code,
            name,
            project,
        } => {
            let id = home.load_identity()?;
            let cfg = home.load_config()?;
            let store = Store::open(&home.db_path())?;
            let peer_bundle = fetch_pairing(&cfg.relay_url, &code)
                .await?
                .ok_or("pairing code not found or expired")?;
            store.upsert_peer(&peer_bundle.peer, &name, peer_bundle.project.as_deref())?;

            let ack_bundle = PairingBundle {
                v: 1,
                code: format!("{code}:ack"),
                peer: id.to_public(),
                project,
                expires_at: now_unix_u64() + 600,
            };
            publish_pairing(&cfg.relay_url, &ack_bundle).await?;
            let _ = ensure_daemon_running(&home).await;
            println!("Paired with '{}' ({})", name, peer_bundle.peer.peer_id);
        }
        Commands::Peers => {
            ensure_daemon_running(&home).await?;
            let cfg = home.load_config()?;
            let token = home.load_token()?;
            let url = format!("http://127.0.0.1:{}/peers", cfg.listen_port);
            let resp = reqwest::Client::new()
                .get(url)
                .bearer_auth(token)
                .send()
                .await?;
            println!("{}", resp.text().await?);
        }
        Commands::Ask {
            target,
            question,
            wait,
            project,
        } => {
            ensure_daemon_running(&home).await?;
            let cfg = home.load_config()?;
            let token = home.load_token()?;
            let url = format!("http://127.0.0.1:{}/ask", cfg.listen_port);
            let resp = reqwest::Client::new()
                .post(url)
                .bearer_auth(token)
                .json(&serde_json::json!({
                    "target": target,
                    "question": question,
                    "wait_seconds": wait,
                    "project": project,
                }))
                .send()
                .await?;
            println!("{}", resp.text().await?);
        }
        Commands::Reply {
            thread_id,
            body,
            requires_human,
        } => {
            ensure_daemon_running(&home).await?;
            let cfg = home.load_config()?;
            let token = home.load_token()?;
            let url = format!("http://127.0.0.1:{}/reply", cfg.listen_port);
            let resp = reqwest::Client::new()
                .post(url)
                .bearer_auth(token)
                .json(&serde_json::json!({
                    "thread_id": thread_id,
                    "body": body,
                    "requires_human": requires_human,
                }))
                .send()
                .await?;
            println!("{}", resp.text().await?);
        }
        Commands::Inbox { unread, thread } => {
            ensure_daemon_running(&home).await?;
            let cfg = home.load_config()?;
            let token = home.load_token()?;
            let mut url = format!(
                "http://127.0.0.1:{}/inbox?unread={}",
                cfg.listen_port,
                if unread { 1 } else { 0 }
            );
            if let Some(tid) = thread {
                url.push_str(&format!("&thread_id={tid}"));
            }
            let resp = reqwest::Client::new()
                .get(url)
                .bearer_auth(token)
                .send()
                .await?;
            println!("{}", resp.text().await?);
        }
        Commands::HookCheck { session, format } => {
            let Ok(cfg) = home.load_config() else {
                return Ok(());
            };
            let Ok(token) = home.load_token() else {
                return Ok(());
            };
            let mut url = format!("http://127.0.0.1:{}/inbox?unread=1", cfg.listen_port);
            if let Some(s) = session {
                url.push_str(&format!("&session={s}"));
            }
            let client = reqwest::Client::new();
            if let Ok(resp) = client
                .get(url)
                .bearer_auth(&token)
                .timeout(Duration::from_millis(500))
                .send()
                .await
            {
                if let Ok(msgs) = resp.json::<Vec<serde_json::Value>>().await {
                    if !msgs.is_empty() {
                        if format == "codex-stop" {
                            // Advance cursor (ack) before handing to Codex Stop hook so it never loops
                            for m in &msgs {
                                if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                                    let _ = client
                                        .post(format!("http://127.0.0.1:{}/inbox/ack", cfg.listen_port))
                                        .bearer_auth(&token)
                                        .json(&serde_json::json!({ "msg_id": id }))
                                        .send()
                                        .await;
                                }
                            }
                            let summary = msgs
                                .iter()
                                .filter_map(|m| {
                                    let from = m.get("from_peer")?.as_str()?;
                                    let body = m.get("body")?.as_str()?;
                                    let tid = m.get("thread_id")?.as_str()?;
                                    Some(format!("[AgentLink from {from} | thread {tid}]: {body}"))
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            println!(
                                "{}",
                                serde_json::json!({
                                    "decision": "block",
                                    "reason": summary
                                })
                            );
                        } else {
                            println!("{}", serde_json::to_string(&msgs)?);
                        }
                    }
                }
            }
        }
        Commands::Mcp => {
            agentlink_mcp::run_mcp_stdio(&home).await?;
        }
        Commands::Setup { dry_run } => {
            let home_dir = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .map(std::path::PathBuf::from)?;
            let results = setup::run_setup(&home_dir, dry_run)?;
            for r in results {
                println!("{:<16} {:<12} {}", r.target, r.status, r.path.display());
            }
            if !dry_run && home.is_initialized() {
                let _ = ensure_daemon_running(&home).await;
            }
        }
        Commands::Block { peer } => {
            let store = Store::open(&home.db_path())?;
            let peer_id = store
                .get_peer(&peer)?
                .map(|p| p.peer_id)
                .unwrap_or(peer.clone());
            store.block_peer(&peer_id)?;
            println!("Blocked peer: {peer} ({peer_id})");
        }
        Commands::Unblock { peer } => {
            let store = Store::open(&home.db_path())?;
            let peer_id = store
                .get_peer(&peer)?
                .map(|p| p.peer_id)
                .unwrap_or(peer.clone());
            store.unblock_peer(&peer_id)?;
            println!("Unblocked peer: {peer} ({peer_id})");
        }
        Commands::Group { action } => {
            let cfg = home.load_config()?;
            let token = home.load_token()?;
            let client = reqwest::Client::new();
            match action {
                GroupAction::Create { name, members } => {
                    let resp = client
                        .post(format!("http://127.0.0.1:{}/groups", cfg.listen_port))
                        .bearer_auth(token)
                        .json(&serde_json::json!({ "name": name, "members": members }))
                        .send()
                        .await?;
                    println!("{}", resp.text().await?);
                }
                GroupAction::List => {
                    let resp = client
                        .get(format!("http://127.0.0.1:{}/groups", cfg.listen_port))
                        .bearer_auth(token)
                        .send()
                        .await?;
                    println!("{}", resp.text().await?);
                }
                GroupAction::Add { group, member } => {
                    let resp = client
                        .post(format!("http://127.0.0.1:{}/groups/members", cfg.listen_port))
                        .bearer_auth(token)
                        .json(&serde_json::json!({ "group": group, "add": member }))
                        .send()
                        .await?;
                    println!("{}", resp.text().await?);
                }
                GroupAction::Remove { group, member } => {
                    let resp = client
                        .post(format!("http://127.0.0.1:{}/groups/members", cfg.listen_port))
                        .bearer_auth(token)
                        .json(&serde_json::json!({ "group": group, "remove": member }))
                        .send()
                        .await?;
                    println!("{}", resp.text().await?);
                }
            }
        }
    }

    Ok(())
}
