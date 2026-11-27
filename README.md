# AgentLink

Lightweight, end-to-end encrypted bridge that lets AI coding agents (**Claude Code, Claude Desktop, Codex, Antigravity**) ask peers—or other agents on the same machine—for missing project context automatically.

## Features
- **Single native Rust binary (`agentlink`):** Cross-platform (Windows, macOS, Linux) with zero runtime dependencies.
- **MCP-native + Skills:** Plugs into Claude Code, Claude Desktop, Codex, and Antigravity via `agentlink setup` or the plugin manifest.
- **Background auto-responder + human escalation:** Answers read-only codebase/plan questions automatically (`claude -p --allowedTools Read,Glob,Grep` / `codex exec --sandbox read-only`) and notifies you (`needs_human`) only when human confirmation is required.
- **Same-laptop & WhatsApp-style groups:** Route `local:<session>` directly over localhost, or create encrypted `group:<name>` rooms mixing remote peers and local sessions.
- **Always-on free relay:** Runs on Cloudflare Workers + Durable Objects (`relay/`).

## Quickstart

### 1. Install `agentlink` (1 command, no Rust required)
- **Windows (PowerShell):**
  ```powershell
  irm https://raw.githubusercontent.com/loyality7/AgentLink/main/scripts/install.ps1 | iex
  ```
- **macOS / Linux:**
  ```bash
  curl -fsSL https://raw.githubusercontent.com/loyality7/AgentLink/main/scripts/install.sh | sh
  ```

### 2. Initialize & Auto-Wire Editors
```bash
agentlink init --name <your-name> --relay https://agent.inafterwords.com
agentlink setup
```
`agentlink setup` configures MCP + hooks for **Claude Code, Claude Desktop, Codex, and Antigravity**, and registers the daemon to auto-start on login.

### 3. Pair With a Friend
```bash
agentlink pair --name <friend-name> --project .
```
Send the generated message to your friend—they can paste it straight into their AI agent.

### Optional: Install as a Plugin
```text
/plugin install agentlink --marketplace loyality7/AgentLink
```

