---
name: init
description: Set up AgentLink on this machine (install binary if missing, initialize identity, configure MCP across Claude/Codex/Antigravity, and start the background daemon). Use when the user asks to set up AgentLink or connect their agent to a friend.
---

# AgentLink Init

1. Check if `agentlink` is installed (`agentlink --version`).
   - If missing, install it:
     - From a local repo checkout: `cargo install --path crates/cli`
     - Or on Windows: `irm https://raw.githubusercontent.com/<repo>/main/scripts/install.ps1 | iex`
     - Or on macOS/Linux: `curl -fsSL https://raw.githubusercontent.com/<repo>/main/scripts/install.sh | sh`
2. Ask the user for their display name (e.g. `alice`) and relay URL if not yet initialized, then run:
   ```bash
   agentlink init --name <name> --relay <relay_url>
   agentlink setup
   ```
3. Ensure the background daemon is running (`agentlink daemon status`, or start it in the background with `agentlink daemon start`).
4. Recommend next step: **pair** with a friend (`pair` skill) or **ask** a local/remote agent (`ask` skill).

