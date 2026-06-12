// Client gateway — the web-backend's relay-facing half, stood up for local dev.
//
// build-relay is device-facing only: it exposes /ws/device + an internal send
// API, and publishes device→app events to the Redis stream relay:device-events.
// The browser never talks to the relay directly. This gateway bridges them:
//
//   browser  --WS-->  gateway  --POST /internal/device/{id}/send-->  relay --> bridge
//   browser  <--WS--  gateway  <--Redis relay:device-events--------  relay <-- bridge
//
// It forwards opaque frames verbatim (it never decrypts) and serves the device's
// transport public key so the client can wrap a session key to it. Routing is by
// session_id, so a client's reconnect (a new session) is just a new mapping —
// the gateway holds no session state that a reconnect could desync.

import { WebSocketServer } from "ws";
import { createClient } from "redis";

const DEVICE_ID = required("DEVICE_ID");
const DEVICE_TRANSPORT_PUB = required("DEVICE_TRANSPORT_PUB");
const RELAY_INTERNAL_URL = process.env.RELAY_INTERNAL_URL || "http://relay:8081";
const REDIS_URL = process.env.REDIS_URL || "redis://redis:6379/0";
const PORT = Number(process.env.GATEWAY_PORT) || 8090;
const STREAM = "relay:device-events";

function required(name) {
  const v = process.env[name];
  if (!v) {
    console.error(`missing required env: ${name}`);
    process.exit(2);
  }
  return v;
}

/** session_id → client WebSocket. */
const sessions = new Map();

const wss = new WebSocketServer({ port: PORT, path: "/ws/client" });

wss.on("connection", (ws) => {
  // Hand the client the device transport key to wrap a session key to.
  ws.send(JSON.stringify({ type: "device_key", transport_public_key: DEVICE_TRANSPORT_PUB }));

  ws.on("message", async (data) => {
    let msg;
    try {
      msg = JSON.parse(data.toString());
    } catch {
      return;
    }
    if (msg.session_id) sessions.set(msg.session_id, ws);

    // Forward client → device through the relay's internal send API.
    try {
      const res = await fetch(`${RELAY_INTERNAL_URL}/internal/device/${DEVICE_ID}/send`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ message: msg }),
      });
      if (!res.ok) {
        console.error("internal_send", res.status, await res.text());
      }
    } catch (e) {
      console.error("internal_send error", e.message);
    }
  });

  ws.on("close", () => {
    for (const [sid, sock] of sessions) if (sock === ws) sessions.delete(sid);
  });
});

console.log(`gateway: client WS on :${PORT}/ws/client → device ${DEVICE_ID}`);

// Device → client: consume the relay's Redis event stream and route by session.
const redis = createClient({ url: REDIS_URL });
redis.on("error", (e) => console.error("redis error", e.message));
await redis.connect();
console.log(`gateway: consuming ${STREAM}`);

let lastId = "$";
for (;;) {
  let res;
  try {
    res = await redis.xRead([{ key: STREAM, id: lastId }], { BLOCK: 5000, COUNT: 100 });
  } catch (e) {
    console.error("xRead error", e.message);
    await new Promise((r) => setTimeout(r, 1000));
    continue;
  }
  if (!res) continue;
  for (const stream of res) {
    for (const { id, message } of stream.messages) {
      lastId = id;
      if (message.device_id !== DEVICE_ID) continue;
      const type = message.type;
      if (type !== "session_accept" && type !== "e2ee_envelope") continue;
      const ws = sessions.get(message.session_id);
      if (!ws || ws.readyState !== ws.OPEN) continue;
      let envelope;
      try {
        envelope = JSON.parse(message.envelope);
      } catch {
        continue;
      }
      ws.send(JSON.stringify({ type, session_id: message.session_id, envelope }));
    }
  }
}
