---
name: init
description: Set up AgentLink on this machine (install binary if missing, initialize identity, configure MCP across Claude/Codex/Antigravity, and start the background daemon). Use when the user asks to set up AgentLink or connect their agent to a friend.
---

# AgentLink Init

1. Check if `agentlink` is installed (`agentlink --version`).
   - If missing, install it from a trusted source:
     - `npm install -g @sarathbabuc/agentlink` (recommended, no Rust required)
     - Or `cargo install --git https://github.com/loyality7/AgentLink agentlink-cli`
     - Or download the prebuilt archive from `https://github.com/loyality7/AgentLink/releases/latest`.
2. Ask the user for their display name (e.g. `alice`) and relay URL if not yet initialized, then run:
   ```bash
   agentlink init --name <name> --relay <relay_url>
   agentlink setup
   ```
3. Ensure the background daemon is running (`agentlink daemon status`, or start it in the background with `agentlink daemon start`).
4. Recommend next step: **pair** with a friend (`pair` skill) or **ask** a local/remote agent (`ask` skill).

