// QA harness: drives the full platform over the E2EE relay and asserts behavior.
//
// Relay-direct topology: dummy-login to the api, mint a gateway token, and
// authenticate straight to the relay's /ws/client (the node gateway is retired).
// Runs the real browser-client logic + transport binding against a live relay +
// bridge. Exercises the task lifecycle (standard + quick), plan/diff inspection,
// merge results, error handling, and parallel tasks.
//
// Prereqs: skriftapp on API_URL (dummy auth enabled), the Rust relay on
// RELAY_URL, and a paired bridge with BRIDGE_QA_AGENT=1 connected to it.
//
// Usage: API_URL=http://127.0.0.1:8090 RELAY_URL=ws://127.0.0.1:18090 node qa.mjs

import WebSocket from "ws";
import * as transport from "@build/secure-transport";
import { openSession } from "./client.mjs";
import { loginWithDummy } from "./skrift-auth.mjs";

const url = process.env.RELAY_URL || "ws://127.0.0.1:18090";
const apiUrl = process.env.API_URL || "http://127.0.0.1:8090";

let passed = 0;
const checks = [];
function check(name, cond, detail = "") {
  checks.push({ name, ok: !!cond, detail });
  if (cond) passed++;
  console.log(`${cond ? "✓" : "✗"} ${name}${detail ? "  — " + detail : ""}`);
}

function connect() {
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
  const ready = new Promise((resolve, reject) => {
    ws.on("open", resolve);
    ws.on("error", reject);
  });
  return { ws, send, recv, ready };
}

async function main() {
  const { mintGatewayToken } = await loginWithDummy(apiUrl);
  const c = connect();
  await c.ready;
  // First frame: authenticate with a gateway token; the relay acks and then
  // pushes device_key for each of our online devices.
  c.send({ type: "authenticate", token: await mintGatewayToken() });
  const ack = await c.recv();
  check("relay accepts the gateway token", ack.type === "authenticated", `got ${ack.type}`);
  const { call } = await openSession({ send: c.send, recv: c.recv, transport });

  // Liveness.
  const pong = await call("ping");
  check("ping round-trips over E2EE", pong.pong === true);

  // Standard task: dispatch → plan → approve → diff → merge.
  const t = await call("task.dispatch", { goal: "Add a greeting banner" });
  check("dispatch reaches plan_review", t.state === "plan_review", `state=${t.state}`);
  check("dispatch returns a build/ branch", /^build\//.test(t.branch), t.branch);

  const plan = await call("task.plan", { task_id: t.task_id });
  check("plan document mentions the goal", plan.contents.includes("Add a greeting banner"));

  const approved = await call("task.approve_plan", { task_id: t.task_id });
  check("approve_plan reaches review", approved.state === "review", `state=${approved.state}`);

  const diff = await call("task.diff", { task_id: t.task_id });
  check(
    "diff shows the produced file",
    diff.files.some((f) => f.path === "result.txt"),
    `${diff.stat.files_changed} files, +${diff.stat.insertions}`
  );
  check("diff patch is non-empty", diff.patch.length > 0);

  const merged = await call("task.approve_merge", { task_id: t.task_id });
  check("approve_merge reaches merged", merged.state === "merged", `state=${merged.state}`);

  // Quick task: dispatch → review → merge (no plan phase).
  const q = await call("task.dispatch", { goal: "Quick fix typo", kind: "quick" });
  check("quick task skips planning (review)", q.state === "review", `state=${q.state}`);
  const qMerged = await call("task.approve_merge", { task_id: q.task_id });
  check("quick task merges", qMerged.state === "merged");

  // Parallel/independent tasks visible on the board.
  const a = await call("task.dispatch", { goal: "Parallel task A" });
  const b = await call("task.dispatch", { goal: "Parallel task B" });
  check("two parallel tasks have distinct branches", a.branch !== b.branch);
  const list = await call("task.list");
  check("task.list reports all tasks", list.tasks.length >= 4, `${list.tasks.length} tasks`);

  // Abandon is safe.
  const abandoned = await call("task.abandon", { task_id: a.task_id });
  check("abandon reaches abandoned", abandoned.state === "abandoned");

  // Error handling: unknown method and missing params are clean errors, not crashes.
  let unknownErrored = false;
  try {
    await call("does.not.exist");
  } catch (e) {
    unknownErrored = /unknown method/.test(e.message);
  }
  check("unknown method returns a clean error", unknownErrored);

  let badParamsErrored = false;
  try {
    await call("task.dispatch", {});
  } catch (e) {
    badParamsErrored = /goal/.test(e.message);
  }
  check("missing param returns a clean error", badParamsErrored);

  c.ws.close();

  const failed = checks.filter((x) => !x.ok);
  console.log(`\n${passed}/${checks.length} checks passed`);
  if (failed.length) {
    console.error("QA FAIL:", failed.map((x) => x.name).join("; "));
    process.exit(1);
  }
  console.log("QA PASS: platform verified end-to-end over E2EE");
  process.exit(0);
}

main().catch((e) => {
  console.error("QA ERROR:", e.message);
  process.exit(1);
});
