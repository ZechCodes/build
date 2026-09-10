// Verifies the four completion criteria for the terminal over E2EE, against the
// live relay — using the SAME TerminalSession the ghostty browser page uses:
//   1. receives updates in real time
//   2. can send feedback (input)
//   3. correctly detects disconnection
//   4. reconnects reliably (snapshot reflects prior state)
//
// Usage: RELAY_URL=ws://localhost:18090 node term-verify.mjs

import WebSocket from "ws";
import * as transport from "@build/secure-transport";
import { TerminalSession } from "./terminal.mjs";
import { openSession } from "./client.mjs";
import { loginWithDummy } from "./skrift-auth.mjs";

const url = process.env.RELAY_URL || "ws://localhost:18090";
const apiUrl = process.env.API_URL || "http://127.0.0.1:8090";
const preferDeviceId = process.env.PREFER_DEVICE_ID || null;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const dec = new TextDecoder();

// Discover a keyed-terminal scope: authenticate, open a request-response session,
// and take the first registered project as the terminal's { project_id } root.
async function discoverScope(mintGatewayToken) {
  const ws = new WebSocket(`${url}/ws/client`);
  const queue = [];
  const waiters = [];
  ws.on("message", (data) => {
    const msg = JSON.parse(data.toString());
    waiters.length ? waiters.shift()(msg) : queue.push(msg);
  });
  const recv = () =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("recv timeout")), 10000);
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
  send({ type: "authenticate", token: await mintGatewayToken() });
  await recv(); // authenticated ack
  const { call } = await openSession({ send, recv, transport, preferDeviceId });
  const projectList = await call("project.list");
  ws.close();
  const projectId = (projectList.projects || [])[0] && projectList.projects[0].project_id;
  if (!projectId) throw new Error("no project registered — cannot scope a terminal");
  return { project_id: projectId };
}

let failures = 0;
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "✓" : "✗"} ${name}${detail ? "  — " + detail : ""}`);
  if (!ok) failures++;
};

async function waitFor(pred, ms, label) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (pred()) return true;
    await sleep(50);
  }
  throw new Error(`timeout waiting for ${label}`);
}

async function main() {
  let output = "";
  let lastSnapshot = "";
  const statuses = [];

  const { mintGatewayToken } = await loginWithDummy(apiUrl, { email: process.env.QA_EMAIL || "qa@localhost" });
  const scope = await discoverScope(mintGatewayToken);
  const term = new TerminalSession({ url, transport, WebSocketImpl: WebSocket, scope, getToken: mintGatewayToken });
  term.onOutput((bytes) => (output += dec.decode(bytes)));
  term.onSnapshot((bytes) => (lastSnapshot = dec.decode(bytes)));
  term.onStatus((s) => statuses.push(s));

  await term.start(100, 30);
  await waitFor(() => statuses.includes("connected"), 8000, "connected");
  await sleep(800); // let the shell emit its prompt

  // (1) + (2): send a command, see its output arrive in real time.
  const marker = "TERM_MARKER_" + Math.random().toString(36).slice(2, 8);
  output = "";
  await term.input(`echo ${marker}\n`);
  await waitFor(() => output.includes(marker), 6000, "live output of marker");
  check("receives updates in real time", output.includes(marker));
  check("can send feedback (input drives the PTY)", output.includes(marker), `echoed ${marker}`);

  // (3): force a disconnect; the session must detect it.
  const statusesBefore = statuses.length;
  term.simulateDrop();
  await waitFor(() => statuses.slice(statusesBefore).includes("disconnected"), 6000, "disconnected status");
  check("correctly detects disconnection", statuses.includes("disconnected"));

  // (4): it reconnects, and the fresh snapshot reflects the prior screen state.
  await waitFor(
    () => statuses.lastIndexOf("connected") > statuses.indexOf("disconnected"),
    12000,
    "reconnected",
  );
  await waitFor(() => lastSnapshot.includes(marker), 8000, "snapshot reflects prior state");
  check("reconnects reliably (snapshot reflects prior state)", lastSnapshot.includes(marker),
    "prior output present after reconnect");

  // Bonus: live output still works after reconnect.
  const marker2 = "AFTER_RECONNECT_" + Math.random().toString(36).slice(2, 8);
  output = "";
  await term.input(`echo ${marker2}\n`);
  await waitFor(() => output.includes(marker2), 6000, "live output after reconnect");
  check("live output works after reconnect", output.includes(marker2));

  await term.close(); // reaps the server PTY — a leaked shell per run trips the 16-terminal cap
  console.log("");
  if (failures) {
    console.error(`TERMINAL QA FAIL: ${failures} check(s) failed`);
    process.exit(1);
  }
  console.log("TERMINAL QA PASS: real-time, input, disconnect detection, and reconnect all verified");
  process.exit(0);
}

main().catch((e) => {
  console.error("TERMINAL QA ERROR:", e.message);
  process.exit(1);
});
