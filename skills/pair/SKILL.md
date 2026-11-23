---
name: pair
description: Pair this AgentLink identity with a friend's agent using a short one-time invite code, or redeem an invite message pasted from a friend.
---

# AgentLink Pair

## Inviting a friend
1. Run `agentlink pair --project . --no-wait` (or without `--no-wait` if waiting in the background).
2. **Always show the generated paste-ready invite paragraph verbatim** so the user can copy-paste it to their friend over chat. Never summarize it away.

## Joining a friend (when user pastes an invite)
1. Extract the **invite code** (`AL-XXXX-XXXX`), **relay URL**, and **peer name** from the pasted message.
2. If AgentLink is not initialized yet, run `agentlink init --name <user-name> --relay <relay_url>` and `agentlink setup`.
3. Run:
   ```bash
   agentlink join <CODE> --name <peer-name> --project .
   ```
4. Confirm both sides are paired (`agentlink peers` or MCP `list_peers`).

