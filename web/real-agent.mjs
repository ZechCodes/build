// Drives the REAL agent happy path over E2EE: write a goal → a real claude
// planning agent authors the plan → approve → a real claude coding agent
// implements it in a run → show the git diff.
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

// Poll an entity (plan or run) until it reaches `target`, over its own get RPC.
async function waitState(call, method, idField, id, target, timeoutMs) {
  const end = Date.now() + timeoutMs;
  let last = "";
  while (Date.now() < end) {
    const t = await call(method, { [idField]: id });
    if (t.state !== last) { console.log(`  → state: ${t.state}${t.summary ? "  (" + t.summary.slice(0, 80) + ")" : ""}`); last = t.state; }
    if (t.state === target) return t;
    if (t.state === "blocked" || t.state === "failed") throw new Error(`${idField} ${t.state}: ${t.summary || t.last_error}`);
    await sleep(2500);
  }
  throw new Error(`timeout waiting for ${target} (last: ${last})`);
}

async function main() {
  const c = connect();
  await c.ready;
  const { call } = await openSession({ send: c.send, recv: c.recv, transport });

  console.log(`\nGOAL: ${goal}\n`);

  // [1] Author the plan at the project level; the planning agent works in a
  // disposable worktree and reports done → plan_review.
  const plan = await call("plan.create", { goal });
  console.log(`planning ${plan.plan_id}`);
  console.log("\n[1] planning agent working…");
  const planned = await waitState(call, "plan.get", "plan_id", plan.plan_id, "plan_review", 240000);

  // Multi-stage plans read from plan.stages/plan.stage_doc; single-doc from plan.doc.
  const multiStage = planned.stages && planned.stages.length > 1;
  if (multiStage) {
    const board = await call("plan.stages", { plan_id: plan.plan_id });
    console.log(`\n=== PLAN (${board.stages.length} stages) ===`);
    for (const s of board.stages) console.log(`  • ${s.id}: ${s.title || ""}`);
    const firstDoc = await call("plan.stage_doc", { plan_id: plan.plan_id, stage_id: board.stages[0].id });
    console.log("\n" + firstDoc.contents.trim().slice(0, 900) + "\n");
  } else {
    const single = await call("plan.doc", { plan_id: plan.plan_id });
    console.log("\n=== PLAN ===\n" + single.contents.trim().slice(0, 900) + "\n");
  }

  // [2] Approve the plan (and every stage's doc), then create a run — the run
  // materializes the plan and the coding agent implements it.
  console.log("[2] approving plan → coding agent implementing…");
  await call("plan.approve", { plan_id: plan.plan_id });
  if (multiStage) {
    const board = await call("plan.stages", { plan_id: plan.plan_id });
    for (const s of board.stages) {
      await call("plan.stage_approve", { plan_id: plan.plan_id, stage_id: s.id });
    }
  } else {
    // A single-doc plan still needs its lone stage approved before a run.
    const board = await call("plan.stages", { plan_id: plan.plan_id }).catch(() => null);
    if (board) {
      for (const s of board.stages) {
        await call("plan.stage_approve", { plan_id: plan.plan_id, stage_id: s.id });
      }
    }
  }

  const run = await call("run.create", { plan_id: plan.plan_id });
  console.log(`dispatched ${run.run_id} on ${run.branch}`);
  // Auto-advance carries any remaining stages through to review in one hop.
  if (multiStage) await call("run.set_auto_advance", { run_id: run.run_id, enabled: true });
  const reviewed = await waitState(call, "run.get", "run_id", run.run_id, "review", 300000);
  void reviewed;

  const diff = await call("run.diff", { run_id: run.run_id });
  console.log(`\n=== DIFF (${diff.stat.files_changed} files, +${diff.stat.insertions}/-${diff.stat.deletions}) ===`);
  for (const f of diff.files) console.log(`  ${f.status.padEnd(8)} ${f.path}`);
  console.log("\n" + diff.patch.split("\n").slice(0, 40).join("\n"));

  console.log("\nREAL AGENT FLOW PASS: goal → plan → approve → run → diff");
  c.ws.close();
  process.exit(0);
}

main().catch((e) => { console.error("REAL AGENT ERROR:", e.message); process.exit(1); });
