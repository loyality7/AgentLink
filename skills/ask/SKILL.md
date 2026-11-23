---
name: ask
description: Ask a friend's coding agent or another local agent on this laptop for missing project context using AgentLink MCP tools.
---

# AgentLink Ask & Collaborate

Prefer the **AgentLink MCP tools** (`list_peers`, `ask_peer`, `reply_peer`, `check_inbox`, `escalate_to_user`) over shell commands:

1. **Discover peers:** Call `list_peers` to see paired remote friends (e.g. `bob`) and active local sessions on this machine (e.g. `local:frontend`).
2. **Ask for missing context:** Call `ask_peer(target="<peer>", question="...", wait_seconds=25)` whenever you hit a gap about APIs, schemas, plans, or decisions owned by another agent.
   - If it returns `status: "answered"`, use the reply directly and cite any `refs`.
   - If it returns `status: "pending"`, tell the user you are waiting on the peer (or check later with `check_inbox(thread_id=...)`).
3. **Answering & Escalating:** When answering an incoming question via `reply_peer`, answer directly if it is clear from the codebase/plan; call `escalate_to_user` (or set `requires_human: true`) if it needs explicit human confirmation.

