// Reconnect verification over the relay.
//
// A deterministic simulated agent streams N ordered chunks at the bridge. The
// client accumulates them via resume-from-seq; we force disconnects and prove the
// client's reconstructed output reconverges *exactly* to the bridge's
// authoritative state (matching checksum, contiguous seqs, no gaps/dupes).
//
// Scenarios:
//   A  disconnect mid-stream, reconnect (new session), resume to completion
//   B  reconnect while fully away (stream finishes during the gap), bulk catch-up
//      — demonstrates proper reconnect load (bounded-batch backlog replay)
//
// Usage: RELAY_URL=ws://localhost:18090 node qa-reconnect.mjs

import { createHash } from "node:crypto";
import WebSocket from "ws";
import * as transport from "@build/secure-transport";
import { openSession } from "./client.mjs";

const url = process.env.RELAY_URL || "ws://localhost:18090";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sha256 = (s) => createHash("sha256").update(s).digest("hex");

let failures = 0;
function check(name, ok, detail = "") {
  console.log(`${ok ? "✓" : "✗"} ${name}${detail ? "  — " + detail : ""}`);
  if (!ok) failures++;
}

/** Connect a fresh client: new WS + new E2EE session. A reconnect is just this again. */
async function connect() {
  const ws = new WebSocket(`${url}/ws/client`);
  const queue = [];
  const waiters = [];
  ws.on("message", (data) => {
    const msg = JSON.parse(data.toString());
    waiters.length ? waiters.shift()(msg) : queue.push(msg);
  });
  const recv = () =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("recv timeout")), 12000);
      const deliver = (m) => {
        clearTimeout(timer);
        resolve(m);
      };
      queue.length ? deliver(queue.shift()) : waiters.push(deliver);
    });
  const send = (obj) => ws.send(JSON.stringify(obj));
  await new Promise((resolve, reject) => {
    ws.on("open", resolve);
    ws.on("error", reject);
  });
  const { call } = await openSession({ send, recv, transport });
  return { call, close: () => ws.close() };
}

/** Pull one bounded batch of events from `since`, appending output text + seqs. */
async function pull(client, streamId, since, texts, seqs, limit = 64) {
  const r = await client.call("stream.events", { stream_id: streamId, since, limit });
  for (const e of r.events) {
    if (e.kind === "output") {
      texts.push(e.data.text);
      seqs.push(e.seq);
    }
  }
  return { since: r.next, head: r.head, complete: r.complete, batch: r.events.length };
}

function verifyContiguous(seqs, n) {
  if (seqs.length !== n) return false;
  for (let i = 0; i < n; i++) if (seqs[i] !== i + 1) return false;
  return true;
}

async function scenarioA() {
  console.log("\n── Scenario A: disconnect mid-stream, reconnect, resume ──");
  const N = 300;
  let c = await connect();
  const { stream_id } = await c.call("stream.start", { count: N, interval_ms: 4 });

  // Read a few batches, then yank the connection mid-stream.
  const texts = [];
  const seqs = [];
  let since = 0;
  for (let i = 0; i < 4; i++) {
    ({ since } = await pull(c, stream_id, since, texts, seqs));
    await sleep(40);
  }
  const collectedBeforeDrop = texts.length;
  check("disconnect happens mid-stream", collectedBeforeDrop > 0 && collectedBeforeDrop < N,
    `${collectedBeforeDrop}/${N} before drop`);
  c.close();
  await sleep(300);

  // Reconnect — brand new session — and resume from the last seq we applied.
  c = await connect();
  for (;;) {
    const r = await pull(c, stream_id, since, texts, seqs);
    since = r.since;
    if (r.complete && since >= r.head) break;
    await sleep(20);
  }

  check("all chunks received after reconnect", texts.length === N, `${texts.length}/${N}`);
  check("seqs contiguous 1..N (no gaps, no dupes)", verifyContiguous(seqs, N));
  const reconstructed = texts.join("\n");
  const st = await c.call("stream.state", { stream_id });
  check("client output matches bridge checksum", st.checksum === sha256(reconstructed));
  check("deterministic content", texts[0] === "chunk-000000" && texts[N - 1] === `chunk-${String(N - 1).padStart(6, "0")}`);
  c.close();
}

async function scenarioB() {
  console.log("\n── Scenario B: reconnect while fully away (bulk catch-up / load) ──");
  const N = 500;
  let c = await connect();
  const { stream_id } = await c.call("stream.start", { count: N, interval_ms: 2 });
  // Leave immediately; the bridge keeps producing into its authoritative log.
  c.close();
  await sleep(N * 2 + 1500); // let the whole stream finish while we're gone

  // Reconnect and replay the entire backlog in bounded batches.
  c = await connect();
  const texts = [];
  const seqs = [];
  let since = 0;
  let batches = 0;
  for (;;) {
    const r = await pull(c, stream_id, since, texts, seqs, 64);
    batches++;
    since = r.since;
    if (r.complete && since >= r.head) break;
  }
  check("entire backlog replayed after being away", texts.length === N, `${texts.length}/${N}`);
  check("seqs contiguous 1..N", verifyContiguous(seqs, N));
  check("backlog came in bounded batches (load chunked)", batches >= Math.ceil(N / 64),
    `${batches} batches of ≤64`);
  const st = await c.call("stream.state", { stream_id });
  check("client output matches bridge checksum", st.checksum === sha256(texts.join("\n")));
  c.close();
}

// `start <count> <interval>`: kick off a stream and print its id, then exit —
// the bridge keeps producing into its authoritative log after we leave. Used by
// the bridge-reconnect scenario, which bounces the relay between start and resume.
async function startMode(count, interval) {
  const c = await connect();
  const { stream_id } = await c.call("stream.start", { count, interval_ms: interval });
  c.close();
  console.log(stream_id);
}

// `resume <stream_id> <count>`: a fresh client resumes from seq 0 and verifies the
// full authoritative output — used after the relay/bridge bounced.
async function resumeMode(streamId, n) {
  const c = await connect();
  const texts = [];
  const seqs = [];
  let since = 0;
  for (;;) {
    const r = await pull(c, streamId, since, texts, seqs);
    since = r.since;
    if (r.complete && since >= r.head) break;
    await sleep(20);
  }
  check("all chunks received after bridge reconnect", texts.length === n, `${texts.length}/${n}`);
  check("seqs contiguous 1..N", verifyContiguous(seqs, n));
  const st = await c.call("stream.state", { stream_id: streamId });
  check("client output matches bridge checksum", st.checksum === sha256(texts.join("\n")));
  c.close();
}

async function main() {
  const [mode, ...args] = process.argv.slice(2);
  if (mode === "start") {
    await startMode(Number(args[0] || 400), Number(args[1] || 10));
    process.exit(0);
  }
  if (mode === "resume") {
    await resumeMode(args[0], Number(args[1] || 400));
  } else {
    await scenarioA();
    await scenarioB();
  }
  console.log("");
  if (failures) {
    console.error(`RECONNECT QA FAIL: ${failures} check(s) failed`);
    process.exit(1);
  }
  console.log("RECONNECT QA PASS: client state reconverged to bridge state across reconnects");
  process.exit(0);
}

main().catch((e) => {
  console.error("RECONNECT QA ERROR:", e.message);
  process.exit(1);
});
