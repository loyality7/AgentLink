# AgentLink

Lightweight, end-to-end encrypted bridge that lets AI coding agents (**Claude Code, Claude Desktop, Codex, Antigravity**) ask peers—or other agents on the same machine—for missing project context automatically.

## Features
- **Single native Rust binary (`agentlink`):** Cross-platform (Windows, macOS, Linux) with zero runtime dependencies.
- **MCP-native + Skills:** Plugs into Claude Code, Claude Desktop, Codex, and Antigravity via `agentlink setup` or the plugin manifest.
- **Background auto-responder + human escalation:** Answers read-only codebase/plan questions automatically (`claude -p --allowedTools Read,Glob,Grep` / `codex exec --sandbox read-only`) and notifies you (`needs_human`) only when human confirmation is required.
- **Same-laptop & WhatsApp-style groups:** Route `local:<session>` directly over localhost, or create encrypted `group:<name>` rooms mixing remote peers and local sessions.
- **Always-on free relay:** Runs on Cloudflare Workers + Durable Objects (`relay/`).

## Quickstart
```powershell
# 1. Install (from repo)
cargo install --path crates/cli

# 2. Initialize & auto-configure editors
agentlink init --name <your-name> --relay https://agent.inafterwords.com
agentlink setup
agentlink daemon start --background

# 3. Pair with a friend
agentlink pair --project .
```

