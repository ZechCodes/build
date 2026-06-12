// Node end-to-end harness: runs the real browser-client logic (client.mjs) and
// the real build-secure-transport JS binding against a live dev relay + bridge.
//
// Proves: browser client → relay → bridge → relay → browser client, fully E2E
// encrypted, with the JS client and the Rust bridge interoperating.
//
// Usage: DEV_RELAY_URL=ws://127.0.0.1:8799 node e2e.mjs

import WebSocket from "ws";
import * as transport from "@build/secure-transport";
import { runClientSession } from "./client.mjs";

const url = process.env.DEV_RELAY_URL || "ws://127.0.0.1:8799";
const ws = new WebSocket(`${url}/ws/client`);

// Adapt the socket to client.mjs's send(obj) / recv()->obj interface.
const queue = [];
const waiters = [];
ws.on("message", (data) => {
  const msg = JSON.parse(data.toString());
  if (waiters.length) waiters.shift()(msg);
  else queue.push(msg);
});
const recv = () =>
  new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("recv timeout")), 10000);
    const deliver = (m) => {
      clearTimeout(timer);
      resolve(m);
    };
    if (queue.length) deliver(queue.shift());
    else waiters.push(deliver);
  });
const send = (obj) => ws.send(JSON.stringify(obj));

await new Promise((resolve, reject) => {
  ws.on("open", resolve);
  ws.on("error", reject);
});

try {
  const payload = await runClientSession({
    send,
    recv,
    transport,
    log: (m) => console.log("[client]", m),
  });
  const expected = JSON.stringify({ echo: { method: "ping", n: 1 }, from: "bridge" });
  if (JSON.stringify(payload) !== expected) {
    console.error("E2E FAIL: unexpected payload", payload);
    process.exit(1);
  }
  console.log("E2E PASS: browser client ↔ relay ↔ bridge round-trip succeeded");
  process.exit(0);
} catch (err) {
  console.error("E2E FAIL:", err.message);
  process.exit(1);
}
