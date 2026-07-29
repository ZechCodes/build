// QA harness: drives the full platform over the E2EE relay and asserts behavior.
//
// Relay-direct topology: dummy-login to the api, mint a gateway token, and
// authenticate straight to the relay's /ws/client (the node gateway is retired).
// Runs the real browser-client logic + transport binding against a live relay +
// bridge. Exercises the Plan/Run split lifecycle: author a project-scoped plan,
// comment + revise its stage docs, approve it, spin up a worktree-scoped run
// (materializing the plan), walk the per-stage validation gate, run-all
// auto-advance, diff inspection, git merge, external-worktree adoption, error
// handling, and parallel runs.
//
// Prereqs: skriftapp on API_URL (dummy auth enabled), the Rust relay on
// RELAY_URL, and a paired bridge with BRIDGE_QA_AGENT=1 connected to it.
//
// Usage: API_URL=http://127.0.0.1:8090 RELAY_URL=ws://127.0.0.1:18090 node qa.mjs

import WebSocket from "ws";
import * as transport from "@build/secure-transport";
import { openSession, openPushSession } from "./client.mjs";
import { loginWithDummy } from "./skrift-auth.mjs";

const b64encode = (s) => Buffer.from(s, "utf8").toString("base64");
const b64decode = (s) => Buffer.from(s || "", "base64").toString("utf8");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function waitFor(pred, ms) {
  const deadline = Date.now() + ms;
  for (;;) {
    const hit = pred();
    if (hit) return hit;
    if (Date.now() >= deadline) return null;
    await sleep(50);
  }
}

const url = process.env.RELAY_URL || "ws://127.0.0.1:18090";
const apiUrl = process.env.API_URL || "http://127.0.0.1:8090";
// With several devices online, pin the one under test (default: first to answer).
const preferDeviceId = process.env.PREFER_DEVICE_ID || null;

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
  const { mintGatewayToken } = await loginWithDummy(apiUrl, { email: process.env.QA_EMAIL || "qa@localhost" });
  const c = connect();
  await c.ready;
  // First frame: authenticate with a gateway token; the relay acks and then
  // pushes device_key for each of our online devices.
  c.send({ type: "authenticate", token: await mintGatewayToken() });
  const ack = await c.recv();
  check("relay accepts the gateway token", ack.type === "authenticated", `got ${ack.type}`);
  const { call } = await openSession({ send: c.send, recv: c.recv, transport, preferDeviceId });

  // Liveness.
  const pong = await call("ping");
  check("ping round-trips over E2EE", pong.pong === true);

  // Standard flow: author a plan → comment/revise its stage docs → approve →
  // create a run (materializes the plan, auto-runs stage 1 to the gate) →
  // walk the per-stage gate → diff → merge.
  const plan = await call("plan.create", { goal: "Add a greeting banner" });
  check("plan.create reaches plan_review", plan.state === "plan_review", `state=${plan.state}`);

  const board = await call("plan.stages", { plan_id: plan.plan_id });
  check("plan arrives as multiple stages", board.stages.length === 2, `${board.stages.length} stages`);
  check(
    "stages start planned",
    board.stages.every((s) => s.state === "planned"),
    board.stages.map((s) => s.state).join(",")
  );
  const [first, second] = board.stages;

  // plan.send_notes is the plan-LEVEL conversation write: the message lands in
  // the plan's thread and the drafting agent revises from it. (It once rejected
  // multi-stage plans as a retired single-doc verb; that guard was deliberately
  // dropped when the plan conversation became persistent, and the plan surface's
  // composer has spoken this verb ever since.) Per-stage comment batches have
  // their own verb, plan.stage_send_notes, exercised just below.
  const planNote = "Keep the second stage reversible.";
  const noted = await call("plan.send_notes", {
    plan_id: plan.plan_id,
    messages: [{ body: planNote, anchor: null }],
  });
  check("plan.send_notes keeps the plan in review", noted.state === "plan_review", `state=${noted.state}`);
  check(
    "plan.send_notes posts the message into the plan thread",
    (noted.thread?.items || []).some(
      (item) => item.type === "message" && item.data.role === "user" && item.data.body === planNote
    ),
    `${(noted.thread?.items || []).length} thread items`
  );

  const doc = await call("plan.stage_doc", { plan_id: plan.plan_id, stage_id: first.id });
  check("stage doc mentions the goal", doc.contents.includes("Add a greeting banner"));

  // Structured comment → batched send → the revision resolves it.
  const added = await call("plan.comment_add", {
    plan_id: plan.plan_id,
    stage_id: first.id,
    body: "Please tighten this step.",
    anchor: { heading_path: ["Stage: First half"], snippet: "Implement the first half" },
  });
  check("comment is minted open", added.comment.state === "open", added.comment.id);
  await call("plan.stage_send_notes", { plan_id: plan.plan_id, stage_id: first.id });
  const afterRevise = await call("plan.stages", { plan_id: plan.plan_id });
  const revisedComment = afterRevise.stages.find((s) => s.id === first.id).comments[0];
  check("revision addresses the comment", revisedComment.state === "addressed", revisedComment.agent_reply);
  const revisedDoc = await call("plan.stage_doc", { plan_id: plan.plan_id, stage_id: first.id });
  check("stage doc was actually revised", revisedDoc.contents.includes("(revised)"));

  // Approve the plan (the coarse gate), then approve only stage 1's doc, then
  // create the run: stage 1 auto-dispatches + validates, resting at the gate.
  const approvedPlan = await call("plan.approve", { plan_id: plan.plan_id });
  check("plan.approve reaches approved", approvedPlan.state === "approved", `state=${approvedPlan.state}`);
  await call("plan.stage_approve", { plan_id: plan.plan_id, stage_id: first.id });

  const run = await call("run.create", { plan_id: plan.plan_id });
  check("run.create rests at the stage gate after stage 1", run.state === "stage_gate", `state=${run.state}`);
  check("run.create returns a build/ branch", /^build\//.test(run.branch), run.branch);
  const firstProgress = run.stages.find((s) => s.id === first.id);
  check("stage 1 validates after its build", firstProgress.state === "validated_passed", firstProgress.state);
  check(
    "validation carries notes for the next stage",
    firstProgress.validation.passed === true && firstProgress.validation.notes_for_next_stage.length > 0,
    firstProgress.validation.notes_for_next_stage
  );

  // The validation gate: stage 2 cannot dispatch until its plan doc is approved.
  // (Stage 1 already validated when the run was created; the remaining gate on
  // the run is the per-stage doc approval — the reinterpreted "stage 2 is gated".)
  let gateErrored = false;
  try {
    await call("run.stage_dispatch", { run_id: run.run_id, stage_id: second.id });
  } catch (e) {
    gateErrored = /not approved/.test(e.message);
  }
  check("stage 2 dispatch is gated on its plan doc approval", gateErrored);

  await call("plan.stage_approve", { plan_id: plan.plan_id, stage_id: second.id });
  const afterSecond = await call("run.stage_dispatch", { run_id: run.run_id, stage_id: second.id });
  check("final stage lands the run in review", afterSecond.state === "review", `state=${afterSecond.state}`);

  const diff = await call("run.diff", { run_id: run.run_id });
  check(
    "diff shows both stages' files",
    diff.files.some((f) => f.path === `result-${first.id}.txt`) &&
      diff.files.some((f) => f.path === `result-${second.id}.txt`),
    `${diff.stat.files_changed} files, +${diff.stat.insertions}`
  );
  check("diff patch is non-empty", diff.patch.length > 0);

  const merged = await call("run.git_action", { run_id: run.run_id, action: "merge" });
  check("git_action merge reaches merged", merged.state === "merged", `state=${merged.state}`);

  // Run-all: approve every stage, create the run, arm auto-advance, and the
  // chain runs to review.
  const runAllPlan = await call("plan.create", { goal: "Run-all banner polish" });
  await call("plan.approve", { plan_id: runAllPlan.plan_id });
  const runAllBoard = await call("plan.stages", { plan_id: runAllPlan.plan_id });
  for (const s of runAllBoard.stages) {
    await call("plan.stage_approve", { plan_id: runAllPlan.plan_id, stage_id: s.id });
  }
  const runAll = await call("run.create", { plan_id: runAllPlan.plan_id });
  const chained = await call("run.set_auto_advance", { run_id: runAll.run_id, enabled: true });
  check("run-all chains every stage to review", chained.state === "review", `state=${chained.state}`);
  check(
    "run-all validates every stage",
    chained.stages.every((s) => s.state === "validated_passed"),
    chained.stages.map((s) => s.state).join(",")
  );
  const runAllMerged = await call("run.git_action", { run_id: runAll.run_id, action: "merge" });
  check("run-all run merges", runAllMerged.state === "merged");

  // External worktree adoption: list → read-only browse → adopt → merge with
  // cleanup=keep. Conditional: runs only when the environment pre-created an
  // external worktree in the project repo (the local harness does this via
  // podman exec); a vanilla stack skips it without failing.
  const withExternals = await call("board.list");
  const external = (withExternals.external_worktrees || []).find((w) => w.adoptable);
  if (external) {
    check("external worktree is listed", true, `${external.branch} (${external.worktree_id})`);
    const browse = await call("worktree.diff", {
      project_id: external.project_id,
      worktree_id: external.worktree_id,
    });
    check("worktree browse shows the dirty diff", browse.patch.length > 0, browse.stat && `+${browse.stat.insertions}`);
    const afterBrowse = await call("board.list");
    check(
      "browsing does not adopt",
      (afterBrowse.external_worktrees || []).some((w) => w.worktree_id === external.worktree_id)
    );

    const adopted = await call("run.adopt", {
      project_id: external.project_id,
      worktree_id: external.worktree_id,
    });
    check("adopt mints a review run", adopted.state === "review" && adopted.adopted === true, `state=${adopted.state}`);
    const afterAdopt = await call("board.list");
    check(
      "adopted worktree leaves the external list",
      !(afterAdopt.external_worktrees || []).some((w) => w.worktree_id === external.worktree_id)
    );

    const adoptedMerged = await call("run.git_action", { run_id: adopted.run_id, action: "merge", cleanup: "keep" });
    check("adopted run merges with cleanup=keep", adoptedMerged.state === "merged", `state=${adoptedMerged.state}`);
  } else {
    console.log("· adoption checks skipped (no external worktree in the project repo)");
  }

  // A run only ever implements a plan: the goal-only dispatch ("Quick task") is
  // gone, so an unplanned session is a claude/codex terminal tab instead.
  let goalOnlyRefused = false;
  try {
    await call("run.create", { goal: "Quick fix typo" });
  } catch (e) {
    goalOnlyRefused = /plan_id/.test(e.message);
  }
  check("a goal without a plan is refused", goalOnlyRefused);

  // Parallel/independent runs visible on the board. `a` is a run we will abandon;
  // `b` is left alive at the stage gate — its worktree and materialized
  // `.build/plan` docs feed the fs/agent surface checks below.
  const aPlan = await call("plan.create", { goal: "Parallel plan A" });
  await call("plan.approve", { plan_id: aPlan.plan_id });
  const aStages = await call("plan.stages", { plan_id: aPlan.plan_id });
  await call("plan.stage_approve", { plan_id: aPlan.plan_id, stage_id: aStages.stages[0].id });
  const a = await call("run.create", { plan_id: aPlan.plan_id });
  const bPlan = await call("plan.create", { goal: "Parallel plan B" });
  await call("plan.approve", { plan_id: bPlan.plan_id });
  const bBoard = await call("plan.stages", { plan_id: bPlan.plan_id });
  await call("plan.stage_approve", { plan_id: bPlan.plan_id, stage_id: bBoard.stages[0].id });
  const b = await call("run.create", { plan_id: bPlan.plan_id });
  check("two parallel runs have distinct branches", a.branch !== b.branch);
  const boardAll = await call("board.list");
  check("board.list reports all runs", boardAll.runs.length >= 4, `${boardAll.runs.length} runs`);

  // Abandon is safe.
  const abandoned = await call("run.abandon", { run_id: a.run_id });
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
    await call("run.create", {});
  } catch (e) {
    badParamsErrored = /plan_id/.test(e.message);
  }
  check("missing param returns a clean error", badParamsErrored);

  // ---- Worktree surfaces (keyed terminals, agent attach, fs browse, primary) ----
  // A dedicated second relay connection mirrors production's terminal socket:
  // request/response terminal RPCs + server-initiated PTY pushes on one session,
  // keeping floods off the main request-response `call` session.
  const projectList = await call("project.list");
  const projectId = (projectList.projects || [])[0] && projectList.projects[0].project_id;
  check("a project is registered for the surface checks", !!projectId, projectId || "none");

  const tc = connect();
  await tc.ready;
  tc.send({ type: "authenticate", token: await mintGatewayToken() });
  const tack = await tc.recv();
  check("terminal socket accepts the gateway token", tack.type === "authenticated");
  const pushes = [];
  const term = await openPushSession({ send: tc.send, recv: tc.recv, transport, preferDeviceId, onPush: (p) => pushes.push(p) });

  if (projectId) {
    // 1. Keyed terminal round-trip in the primary scope.
    const created = await term.call("term.create", { project_id: projectId, cols: 80, rows: 24 });
    check("term.create mints a keyed id", /^term-\d+$/.test(created.term_id || ""), created.term_id);
    const attached = await term.call("term.attach", { term_id: created.term_id, cols: 80, rows: 24 });
    check(
      "term.attach returns a snapshot + numeric cursor",
      typeof attached.snapshot === "string" && typeof attached.cursor === "number",
      `cursor=${attached.cursor}`,
    );
    const marker = `qa-term-${Date.now()}`;
    await term.call("term.input", { term_id: created.term_id, data: b64encode(`echo ${marker}\r`) });
    const echoed = await waitFor(() => {
      const text = pushes
        .filter((p) => (p.type === "term.output" || p.type === "term.reset") && p.term_id === created.term_id)
        .map((p) => b64decode(p.data))
        .join("");
      return text.includes(marker) ? text : null;
    }, 10000);
    check("terminal echoes input back over the relay", !!echoed);
    const listed = await term.call("term.list", { project_id: projectId });
    check("term.list includes the open terminal", (listed.terminals || []).some((t2) => t2.term_id === created.term_id));
    await term.call("term.close", { term_id: created.term_id });
    const listed2 = await term.call("term.list", { project_id: projectId });
    check("term.close removes it from term.list", !(listed2.terminals || []).some((t2) => t2.term_id === created.term_id));

    // 4. Primary-changes summary + project.diff shape.
    const withPrimary = await call("board.list");
    const pc = (withPrimary.primary_changes || []).find((p) => p.project_id === projectId);
    check(
      "board.list.primary_changes carries a branch + numeric files_changed",
      !!pc && typeof pc.branch === "string" && pc.branch.length > 0 && typeof pc.files_changed === "number",
      pc ? `${pc.branch} (${pc.files_changed})` : "missing",
    );
    const pd = await call("project.diff", { project_id: projectId });
    check(
      "project.diff returns the stat/files/patch shape",
      pd && pd.stat && Array.isArray(pd.files) && typeof pd.patch === "string",
      pd && pd.stat && `${pd.stat.files_changed} files`,
    );
  }

  // 2 + 3. fs round-trip and fencing, against a live run's worktree. `b`
  // (Parallel run B) rests at the stage gate — its worktree and the materialized
  // `.build/plan` stage docs exist on disk (the standard run merged+pruned, `a`
  // was abandoned). fs scopes on `run_id` now (there is no plan-worktree scope).
  const tree = await call("fs.tree", { run_id: b.run_id });
  const names = (tree.entries || []).map((e) => e.name);
  check("fs.tree lists .build and hides .git", names.includes(".build") && !names.includes(".git"), names.join(","));
  const bStages = await call("plan.stages", { plan_id: bPlan.plan_id });
  const firstStage = bStages.stages[0];
  const stageFile = await call("fs.read", { run_id: b.run_id, path: firstStage.path });
  const stageDoc = await call("plan.stage_doc", { plan_id: bPlan.plan_id, stage_id: firstStage.id });
  check(
    "fs.read returns the stage doc's exact bytes",
    b64decode(stageFile.content_b64) === stageDoc.contents,
    `${stageFile.size} bytes, mime=${stageFile.mime}`,
  );
  let fenceErrored = false;
  try {
    await call("fs.read", { run_id: b.run_id, path: "../../../etc/passwd" });
  } catch (e) {
    fenceErrored = /path escapes/.test(e.message);
  }
  check("fs.read fences a traversal path", fenceErrored);

  // 5. Agent attach: you address it by the opaque entity `id` (plan-… / run-…),
  // but what comes back is keyed by the WORKTREE — `agent:<worktree_id>`, a hash
  // of the canonical root — because an agent belongs to a directory, not to an
  // entity. An entity-keyed id would let two entities over one root address two
  // different agents, which is the whole thing the tab primitive rules out. So
  // the id must be stable across attaches and must NOT be the entity's own id.
  const agentLive = await term.call("agent.attach", { id: b.run_id });
  const agentAgain = await term.call("agent.attach", { id: b.run_id });
  check(
    "agent.attach returns a worktree-keyed id + boolean live",
    agentLive.term_id.startsWith("agent:") &&
      agentLive.term_id !== `agent:${b.run_id}` &&
      agentLive.term_id === agentAgain.term_id &&
      typeof agentLive.live === "boolean",
    `term_id=${agentLive.term_id} live=${agentLive.live}`,
  );
  const agentMerged = await term.call("agent.attach", { id: run.run_id });
  check("agent.attach on a merged run succeeds with live:false", agentMerged.live === false, `live=${agentMerged.live}`);

  tc.ws.close();
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
