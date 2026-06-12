// Drives the REAL agent happy path over E2EE: write a goal → a real claude
// planning agent writes the plan → approve → a real claude coding agent
// implements it → show the git diff.
//
// Usage: RELAY_URL=ws://localhost:18090 node real-agent.mjs ["goal"]

import WebSocket from "ws";
import * as transport from "@build/secure-transport";
import { openSession } from "./client.mjs";

const url = process.env.RELAY_URL || "ws://localhost:18090";
const goal =
  process.argv[2] ||
  "Add a hello() function to a new file greeting.py that returns 'Hello, World!', with a pytest test.";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function connect() {
  const ws = new WebSocket(`${url}/ws/client`);
  const q = [], w = [];
  ws.on("message", (d) => { const m = JSON.parse(d.toString()); w.length ? w.shift()(m) : q.push(m); });
  const recv = () => new Promise((res) => (q.length ? res(q.shift()) : w.push(res)));
  const send = (o) => ws.send(JSON.stringify(o));
  const ready = new Promise((res, rej) => { ws.on("open", res); ws.on("error", rej); });
  return { ws, send, recv, ready };
}

async function waitState(call, taskId, target, timeoutMs) {
  const end = Date.now() + timeoutMs;
  let last = "";
  while (Date.now() < end) {
    const t = await call("task.get", { task_id: taskId });
    if (t.state !== last) { console.log(`  → state: ${t.state}${t.summary ? "  (" + t.summary.slice(0, 80) + ")" : ""}`); last = t.state; }
    if (t.state === target) return t;
    if (t.state === "blocked" || t.state === "failed") throw new Error(`task ${t.state}: ${t.summary}`);
    await sleep(2500);
  }
  throw new Error(`timeout waiting for ${target} (last: ${last})`);
}

async function main() {
  const c = connect();
  await c.ready;
  const { call } = await openSession({ send: c.send, recv: c.recv, transport });

  console.log(`\nGOAL: ${goal}\n`);
  const t = await call("task.dispatch", { goal });
  console.log(`dispatched ${t.task_id} on ${t.branch}`);

  console.log("\n[1] planning agent working…");
  await waitState(call, t.task_id, "plan_review", 240000);
  const plan = await call("task.plan", { task_id: t.task_id });
  console.log("\n=== PLAN ===\n" + plan.contents.trim().slice(0, 900) + "\n");

  console.log("[2] approving plan → coding agent implementing…");
  await call("task.approve_plan", { task_id: t.task_id });
  await waitState(call, t.task_id, "review", 300000);

  const diff = await call("task.diff", { task_id: t.task_id });
  console.log(`\n=== DIFF (${diff.stat.files_changed} files, +${diff.stat.insertions}/-${diff.stat.deletions}) ===`);
  for (const f of diff.files) console.log(`  ${f.status.padEnd(8)} ${f.path}`);
  console.log("\n" + diff.patch.split("\n").slice(0, 40).join("\n"));

  console.log("\nREAL AGENT FLOW PASS: goal → plan → approve → build → diff");
  c.ws.close();
  process.exit(0);
}

main().catch((e) => { console.error("REAL AGENT ERROR:", e.message); process.exit(1); });
