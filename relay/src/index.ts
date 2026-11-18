import { DurableObject } from "cloudflare:workers";

export interface Env {
  MAILBOX: DurableObjectNamespace<MailboxDO>;
}

interface EncryptedEnvelope {
  v: number;
  id: string;
  from: string;
  to: string;
  ephemeral_pk: string;
  nonce: string;
  ciphertext: string;
  signature: string;
}

type RelayFrame =
  | { op: "auth"; peer_id: string; timestamp: number; sign_pk: string; signature: string }
  | { op: "auth_ok"; peer_id: string }
  | { op: "send"; envelope: EncryptedEnvelope }
  | { op: "deliver"; envelope: EncryptedEnvelope }
  | { op: "ack"; msg_id: string }
  | { op: "error"; message: string };

interface WsAttachment {
  peerId: string;
  authed: boolean;
}

const AUTH_SKEW_SECS = 300;
const MAX_ENVELOPE_BYTES = 262_144; // 256 KB
const MAX_INBOX_MESSAGES = 500;

function b64ToBytes(b64: string): Uint8Array<ArrayBuffer> {
  const bin = atob(b64);
  const out = new Uint8Array(new ArrayBuffer(bin.length));
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

async function verifyEd25519(signPkB64: string, message: string, sigB64: string): Promise<boolean> {
  try {
    const key = await crypto.subtle.importKey(
      "raw",
      b64ToBytes(signPkB64),
      { name: "Ed25519" },
      false,
      ["verify"]
    );
    return await crypto.subtle.verify(
      "Ed25519",
      key,
      b64ToBytes(sigB64),
      new TextEncoder().encode(message)
    );
  } catch {
    return false;
  }
}

export class MailboxDO extends DurableObject<Env> {
  private initSchema() {
    this.ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS inbox (
        id TEXT PRIMARY KEY,
        envelope TEXT NOT NULL,
        created_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS pairing (
        code TEXT PRIMARY KEY,
        payload TEXT NOT NULL,
        expires_at INTEGER NOT NULL
      );
    `);
  }

  async fetch(request: Request): Promise<Response> {
    this.initSchema();
    const url = new URL(request.url);

    if (url.pathname === "/ws") {
      const upgrade = request.headers.get("Upgrade");
      if (upgrade?.toLowerCase() !== "websocket") {
        return new Response("Expected WebSocket", { status: 426 });
      }
      const peerId = url.searchParams.get("peer_id") ?? "";
      const pair = new WebSocketPair();
      const [client, server] = Object.values(pair);

      this.ctx.acceptWebSocket(server);
      server.serializeAttachment({ peerId, authed: false } satisfies WsAttachment);

      return new Response(null, { status: 101, webSocket: client });
    }

    if (url.pathname === "/deliver" && request.method === "POST") {
      const rawBody = await request.text();
      if (rawBody.length > MAX_ENVELOPE_BYTES) {
        return new Response("Payload too large", { status: 413 });
      }
      const countRow = this.ctx.storage.sql
        .exec<{ cnt: number }>("SELECT COUNT(*) AS cnt FROM inbox")
        .one();
      if (countRow.cnt >= MAX_INBOX_MESSAGES) {
        return new Response("Mailbox full", { status: 429 });
      }

      const envelope = JSON.parse(rawBody) as EncryptedEnvelope;
      const serialized = rawBody;
      const now = Math.floor(Date.now() / 1000);

      this.ctx.storage.sql.exec(
        "INSERT OR REPLACE INTO inbox (id, envelope, created_at) VALUES (?, ?, ?)",
        envelope.id,
        serialized,
        now
      );

      for (const ws of this.ctx.getWebSockets()) {
        const att = ws.deserializeAttachment() as WsAttachment | null;
        if (att?.authed) {
          ws.send(JSON.stringify({ op: "deliver", envelope } satisfies RelayFrame));
        }
      }

      return Response.json({ ok: true });
    }

    if (url.pathname === "/pair" && request.method === "POST") {
      const body = (await request.json()) as { code: string; payload: string; expires_at: number };
      const now = Math.floor(Date.now() / 1000);
      this.ctx.storage.sql.exec("DELETE FROM pairing WHERE expires_at < ?", now);
      this.ctx.storage.sql.exec(
        "INSERT OR REPLACE INTO pairing (code, payload, expires_at) VALUES (?, ?, ?)",
        body.code,
        body.payload,
        body.expires_at
      );
      return Response.json({ ok: true });
    }

    if (url.pathname.startsWith("/pair/") && request.method === "GET") {
      const code = decodeURIComponent(url.pathname.slice("/pair/".length));
      const now = Math.floor(Date.now() / 1000);
      const rows = this.ctx.storage.sql
        .exec<{ payload: string; expires_at: number }>(
          "SELECT payload, expires_at FROM pairing WHERE code = ?",
          code
        )
        .toArray();
      if (rows.length === 0 || rows[0].expires_at < now) {
        return new Response("Not found", { status: 404 });
      }
      return new Response(rows[0].payload, {
        headers: { "Content-Type": "application/json" },
      });
    }

    return new Response("Not found", { status: 404 });
  }

  async webSocketMessage(ws: WebSocket, raw: string | ArrayBuffer) {
    this.initSchema();
    const text = typeof raw === "string" ? raw : new TextDecoder().decode(raw);
    let frame: RelayFrame;
    try {
      frame = JSON.parse(text) as RelayFrame;
    } catch {
      ws.send(JSON.stringify({ op: "error", message: "invalid json" } satisfies RelayFrame));
      return;
    }

    const att = (ws.deserializeAttachment() as WsAttachment | null) ?? {
      peerId: "",
      authed: false,
    };

    if (frame.op === "auth") {
      const now = Math.floor(Date.now() / 1000);
      if (Math.abs(now - frame.timestamp) > AUTH_SKEW_SECS) {
        ws.send(JSON.stringify({ op: "error", message: "auth timestamp expired" }));
        ws.close(1008, "auth expired");
        return;
      }
      const canonical = `auth:${frame.peer_id}:${frame.timestamp}`;
      const ok = await verifyEd25519(frame.sign_pk, canonical, frame.signature);
      if (!ok) {
        ws.send(JSON.stringify({ op: "error", message: "invalid auth signature" }));
        ws.close(1008, "invalid signature");
        return;
      }

      ws.serializeAttachment({ peerId: frame.peer_id, authed: true } satisfies WsAttachment);
      ws.send(JSON.stringify({ op: "auth_ok", peer_id: frame.peer_id } satisfies RelayFrame));

      const pending = this.ctx.storage.sql
        .exec<{ envelope: string }>("SELECT envelope FROM inbox ORDER BY created_at ASC")
        .toArray();
      for (const row of pending) {
        const envelope = JSON.parse(row.envelope) as EncryptedEnvelope;
        ws.send(JSON.stringify({ op: "deliver", envelope } satisfies RelayFrame));
      }
      return;
    }

    if (!att.authed) {
      ws.send(JSON.stringify({ op: "error", message: "unauthenticated" } satisfies RelayFrame));
      return;
    }

    if (frame.op === "send") {
      if (frame.envelope.from !== att.peerId) {
        ws.send(JSON.stringify({ op: "error", message: "sender mismatch" } satisfies RelayFrame));
        return;
      }
      const targetId = this.env.MAILBOX.idFromName(frame.envelope.to);
      const target = this.env.MAILBOX.get(targetId);
      await target.fetch("https://do/deliver", {
        method: "POST",
        body: JSON.stringify(frame.envelope),
      });
      return;
    }

    if (frame.op === "ack") {
      this.ctx.storage.sql.exec("DELETE FROM inbox WHERE id = ?", frame.msg_id);
    }
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === "/health") {
      return Response.json({ ok: true, service: "agentlink-relay" });
    }

    if (url.pathname === "/ws") {
      const peerId = url.searchParams.get("peer_id");
      if (!peerId) {
        return new Response("Missing peer_id", { status: 400 });
      }
      const stub = env.MAILBOX.get(env.MAILBOX.idFromName(peerId));
      return stub.fetch(request);
    }

    if (url.pathname === "/pair" || url.pathname.startsWith("/pair/")) {
      const stub = env.MAILBOX.get(env.MAILBOX.idFromName("__pairing__"));
      return stub.fetch(request);
    }

    return new Response("Not found", { status: 404 });
  },
};

