# AgentLink

**End-to-end encrypted agent-to-agent bridge for AI coding assistants.**

AgentLink lets coding agents (**Claude Code, Claude Desktop, OpenAI Codex, and Antigravity**) ask peers—or other agents running on the same machine—for missing project context automatically, with human-in-the-loop escalation only when decisions are required.

---

## Why AgentLink?

When developers collaborate with AI coding agents, one person often holds the broader architecture or spec while the other receives partial context. Instead of acting as a human copy-paste bridge:

- **Cross-machine E2EE messaging:** Your agent asks your peer's agent directly over an untrusted WebSocket relay (`X25519` + `XChaCha20-Poly1305` + `Ed25519`).
- **Background auto-responder + human escalation:** Routine codebase/plan questions are answered automatically in the background (read-only sandboxed `claude -p` or `codex exec`). Decisions, approvals, and sensitive topics trigger a desktop notification (`needs_human`).
- **Same-laptop multi-agent routing:** Run two agents on your own machine (e.g., backend + frontend) and route between them over localhost in `<1 ms`.
- **WhatsApp-style groups:** Create encrypted `group:<name>` rooms mixing remote peers and local sessions.
- **Single native Rust binary:** Fast, lightweight (~5 MB idle RAM), zero runtime dependencies on Windows, macOS, and Linux.

---

## Installation

### Option 1: Prebuilt Binary (GitHub Releases — No Rust Required)
Download the archive for your OS from [Releases](https://github.com/loyality7/AgentLink/releases/latest) and place `agentlink` (or `agentlink.exe`) on your `PATH`:
- Windows: `agentlink-x86_64-pc-windows-msvc.zip`
- macOS (Apple Silicon): `agentlink-aarch64-apple-darwin.tar.gz`
- Linux (x64): `agentlink-x86_64-unknown-linux-gnu.tar.gz`

### Option 2: npm (after publishing `npm/` to npm)
```bash
npm install -g agentlink-cli
```

### Option 3: From Source (Rust)
```bash
cargo install --git https://github.com/loyality7/AgentLink agentlink-cli
```

### Optional: Editor Plugin
- **Claude Code:** `/plugin install agentlink --marketplace loyality7/AgentLink`
- **Codex:** `codex plugin marketplace add loyality7/AgentLink && codex plugin add agentlink@agentlink`

---

## Setup

### 1. Deploy Your Free Relay (One Time, ~1 Min)
AgentLink uses a lightweight **Cloudflare Workers + Durable Objects** relay (free tier, always-on, stores only ciphertext until delivered):

```bash
cd relay
npm install
npx wrangler login
npx wrangler deploy
```

Note your deployed relay URL (e.g. `https://your-relay.example.com`).

### 2. Initialize & Configure Your Editors
```bash
agentlink init --name <your-name> --relay <YOUR_RELAY_URL>
agentlink setup
```

`agentlink setup` automatically:
- Registers the `agentlink mcp` server in **Claude Code, Claude Desktop, Codex, and Antigravity**.
- Configures live-session wake hooks (`UserPromptSubmit` / `Stop`) for Claude Code and Codex.
- Registers the background daemon to auto-start on OS login.

### 3. Pair With a Peer
```bash
agentlink pair --name <peer-name> --project .
```
Send the generated invite message to your peer. They paste it into their agent (or run `agentlink join <CODE> --name <your-name> --project .`), and both sides pin each other's public keys automatically.

---

## MCP Tools

Once configured, your AI agents have access to:
- `list_peers` — List paired remote peers, active local sessions (`local:<name>`), and groups (`group:<name>`).
- `ask_peer` — Ask a peer, local session, or group a question and optionally wait inline for the reply.
- `reply_peer` — Reply to an incoming thread.
- `check_inbox` — Read pending questions or replies.
- `escalate_to_user` — Flag a thread as `needs_human` and trigger an OS desktop notification.

---

## Project Guardrails (`.agentlink.json`)

Place `.agentlink.json` in a repository root to control what the background responder can read:

```json
{
  "project": "my-app",
  "mode": "auto",
  "responder": "claude",
  "allowed_paths": ["src/**", "docs/**", "README.md"],
  "blocked_paths": [".env*", "**/*.pem", "**/*.key", "**/secrets/**"],
  "max_hops": 5,
  "require_human_for": [
    "architecture changes",
    "modifying or deleting files",
    "sharing credentials or API keys"
  ]
}
```

---

## CLI Reference

| Command | Description |
| --- | --- |
| `agentlink init --name <n> --relay <url>` | Create identity keys and local config |
| `agentlink setup` | Configure MCP, editor hooks, and OS auto-start |
| `agentlink pair --name <p> --project .` | Create an `AL-XXXX-XXXX` invite code and watch in background |
| `agentlink join <code> --name <p>` | Redeem a peer's invite code |
| `agentlink peers` | Show remote peers, local sessions, and groups |
| `agentlink ask <target> "<question>"` | Ask a peer, `local:<session>`, or `group:<name>` |
| `agentlink reply <thread_id> "<answer>"` | Reply to a thread |
| `agentlink inbox [--unread]` | View messages |
| `agentlink group create <name> --members a,b` | Create an encrypted group |
| `agentlink block <peer>` / `unblock <peer>` | Drop messages from a peer before decryption |
| `agentlink daemon start [--background] \| stop \| status` | Manage the background daemon |

---

## License

MIT
