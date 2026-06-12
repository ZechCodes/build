// Performance probe for the terminal over E2EE, against the full real path
// (browser-client logic → gateway → real relay → bridge). Measures:
//   - keystroke→echo round-trip latency (idle and under heavy output load)
//   - sustained output throughput: bytes/sec, frames/sec, mean frame size
//
// Usage: RELAY_URL=ws://localhost:18090 node term-perf.mjs

import WebSocket from "ws";
import * as transport from "@build/secure-transport";
import { TerminalSession } from "./terminal.mjs";

const url = process.env.RELAY_URL || "ws://localhost:18090";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const dec = new TextDecoder();

let bytes = 0;
let frames = 0;
let buf = "";

const session = new TerminalSession({ url, transport, WebSocketImpl: WebSocket });
session.onOutput((b) => {
  bytes += b.length;
  frames++;
  buf += dec.decode(b);
  if (buf.length > 1 << 16) buf = buf.slice(-(1 << 15)); // keep recent tail
});
session.onSnapshot(() => {});

async function waitFor(pred, ms, label) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (pred()) return;
    await sleep(5);
  }
  throw new Error(`timeout: ${label}`);
}

let n = 0;
async function rtt() {
  const m = `RTT_${++n}_${Math.random().toString(36).slice(2, 7)}`;
  const t0 = performance.now();
  await session.input(`printf '${m}\\n'\n`);
  await waitFor(() => buf.includes(m), 6000, "rtt marker");
  return performance.now() - t0;
}

function stats(xs) {
  const s = [...xs].sort((a, b) => a - b);
  const at = (p) => s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))];
  const mean = s.reduce((a, b) => a + b, 0) / s.length;
  return { min: s[0], p50: at(50), p95: at(95), max: s[s.length - 1], mean };
}
const ms = (x) => `${x.toFixed(1)}ms`;
const fmtBytes = (b) => (b > 1 << 20 ? `${(b / (1 << 20)).toFixed(1)}MB` : `${(b / 1024).toFixed(0)}KB`);

async function main() {
  await session.start(120, 40);
  await waitFor(() => buf !== "" || true, 8000, "connect");
  await sleep(1200); // shell prompt

  // 1) Idle keystroke→echo latency.
  const idle = [];
  for (let i = 0; i < 25; i++) idle.push(await rtt());
  const I = stats(idle);
  console.log(`\nidle keystroke RTT:   p50 ${ms(I.p50)}  p95 ${ms(I.p95)}  mean ${ms(I.mean)}  (min ${ms(I.min)}, max ${ms(I.max)})`);

  // 2) Sustained throughput. The sentinel is split by '' so it only matches in
  //    OUTPUT, never in the echoed command line.
  const N = Number(process.env.FLOOD_LINES || 500000);
  bytes = 0; frames = 0; buf = "";
  const t0 = performance.now();
  let disconnected = false;
  const onDrop = () => (disconnected = true);
  session.onStatus((s) => s === "disconnected" && onDrop());
  await session.input(`seq 1 ${N}; printf 'FLU''SH_DONE\\n'\n`);
  try {
    await waitFor(() => buf.includes("FLUSH_DONE") || disconnected, 60000, "flood done");
  } catch { /* timeout */ }
  const dt = (performance.now() - t0) / 1000;
  const mbps = bytes / (1 << 20) / dt;
  console.log(
    `\nthroughput (${N} lines): ${fmtBytes(bytes)} in ${dt.toFixed(2)}s` +
      `  =>  ${mbps.toFixed(1)} MB/s,  ${(frames / dt).toFixed(0)} frames/s,  mean frame ${frames ? (bytes / frames).toFixed(0) : 0}B`,
  );
  if (disconnected) console.log("  ⚠ DISCONNECTED under flood (head-of-line / saturation)");
  await sleep(1500);

  // 3) Realistic TUI load: full-screen redraws (~30 fps for ~3s, like htop/vim),
  //    measuring both delivered frame rate and keystroke latency *during* it.
  if (!disconnected) {
    await sleep(800);
    bytes = 0; frames = 0; buf = "";
    const t1 = performance.now();
    await session.input(`( for i in $(seq 1 90); do printf '\\033[2J\\033[H'; seq 1 35; sleep 0.03; done; printf 'TU''I_DONE\\n' ) &\n`).catch(() => {});
    await sleep(300);
    const loaded = [];
    try {
      for (let i = 0; i < 12; i++) loaded.push(await rtt());
      const L = stats(loaded);
      const tdt = (performance.now() - t1) / 1000;
      console.log(`TUI redraw load:       ${(frames / tdt).toFixed(0)} frames/s delivered, mean frame ${frames ? (bytes / frames).toFixed(0) : 0}B`);
      console.log(`keystroke RTT @ load:  p50 ${ms(L.p50)}  p95 ${ms(L.p95)}  mean ${ms(L.mean)}  (max ${ms(L.max)})`);
    } catch (e) {
      console.log(`under TUI load:        ⚠ ${e.message}`);
    }
    await waitFor(() => buf.includes("TUI_DONE"), 15000, "tui done").catch(() => {});
  }

  console.log("\n(full path: client → gateway → real relay → Redis → bridge, E2EE end-to-end)");
  session.close();
  process.exit(0);
}

main().catch((e) => {
  console.error("PERF ERROR:", e.message);
  process.exit(1);
});
