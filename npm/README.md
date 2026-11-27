# AgentLink (`@sarathbabuc/agentlink`)

**End-to-end encrypted agent-to-agent bridge for AI coding assistants.**

AgentLink lets coding agents (**Claude Code, Claude Desktop, OpenAI Codex, and Antigravity**) ask peers—or other agents running on the same machine—for missing project context automatically, with human-in-the-loop escalation when decisions are required.

## Install
```bash
npm install -g @sarathbabuc/agentlink
```

## Quickstart
```bash
# 1. Initialize & auto-wire editors (Claude, Codex, Antigravity)
agentlink init --name <your-name> --relay <YOUR_RELAY_URL>
agentlink setup

# 2. Pair with a peer
agentlink pair --name <peer-name> --project .
```

For full documentation, relay setup, `.agentlink.json` guardrails, and source code, visit:
https://github.com/loyality7/AgentLink
