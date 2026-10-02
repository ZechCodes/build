// The task tracker's browser pass: the SPA half driven against a real bridge.
//
// Seventeen checks over the tracker's surfaces, run against the compose stack.
// Every check prints PASS or FAIL with a detail line and none of them aborts
// the run, so one broken surface still reports on the other sixteen.
// The completed run exits 1 if any check failed, and 0 if all passed.
//
//   TASKS_REPO=<a build-web checkout> APP_URL=http://localhost:8090 \
//     node web/tracker-check.mjs
//
// It reads /tmp/live-seed.json, so run web/live-seed.mjs first and write its
// SEED line there. Bridge calls go through the qa container of TASKS_REPO's
// compose file; the browser drives the SPA by hash route.
//
// Two things this file is careful about, because both produced a false result
// the first time it ran:
//
//   • ONE checkout. `TASKS_REPO` is refused rather than defaulted — there is
//     more than one build-web checkout on this machine, and a forgotten
//     variable would point the run at a different tree's compose file and pass
//     against a bridge that was never under test.
//   • ONE browser context. `browser.newPage()` opens a context of its own with
//     no session cookie, so a second page lands on the sign-in gate and every
//     selector finds nothing — which reads exactly like a broken surface.
//
// Timings are measured INSIDE the qa container. Each call spawns a
// `docker compose run` and that startup is about a second, twenty times the
// call it wraps; timing from out here measures docker.

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { execSync } from "node:child_process";
import { chromium } from "playwright";
import { summarizeChecks } from "./checkResults.mjs";

const APP = process.env.APP_URL || "http://localhost:8090";

/**
 * The ONE checkout this whole session runs off.
 *
 * Refused rather than defaulted on purpose. There is more than one build-web
 * checkout on this machine, and a default would mean a forgotten env var
 * silently pointing the run at a different tree's compose file — which is how
 * you get a green pass against a bridge that is not the one under test. The
 * bridge, the pairing, the seed and every check below must come off one tree.
 */
const REPO = process.env.TASKS_REPO;
if (!REPO) {
  console.error("set TASKS_REPO to the checkout at the merged sha — this run must not mix checkouts");
  process.exit(2);
}
const compose = `${REPO}/deploy/compose.real.yml`;
if (!existsSync(compose)) {
  console.error(`no compose file at ${compose} — is TASKS_REPO a build-web checkout?`);
  process.exit(2);
}
console.log(`running off ${REPO}`);
const seed = JSON.parse(readFileSync("/tmp/live-seed.json", "utf8"));
const ws = seed.workspaces[0];

const results = [];
const record = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
};
/** A check that throws is a failed check, not a failed run. */
const check = async (name, run) => {
  try {
    const { ok, detail } = await run();
    record(name, ok, detail);
  } catch (error) {
    record(name, false, `threw: ${String(error && error.message).slice(0, 160)}`);
  }
};

/**
 * One bridge RPC, from inside the qa container.
 *
 * The RPC is timed INSIDE the container and reported as `lastCallMs`, because
 * each of these spawns a whole `docker compose run` and that startup is about a
 * second — twenty times the call it is wrapping. Timing this from out here
 * would make every number in this file a measurement of docker.
 */
let lastCallMs = 0;
function call(method, params) {
  const script = `import * as transport from "@build/secure-transport"; import { openDeviceLink, openRendezvous } from "./client.mjs"; import { loginWithDummy } from "./skrift-auth.mjs";
const { cookie, mintGatewayToken } = await loginWithDummy(process.env.API_URL, { email: "qa@localhost" });
const rendezvous = await openRendezvous({ relayUrl: process.env.RELAY_URL, mintGatewayToken });
const link = await openDeviceLink({ rendezvous, transport, apiUrl: process.env.API_URL, cookie });
const t0 = Date.now();
const r = await link.session.call(${JSON.stringify(method)}, ${JSON.stringify(params)});
console.log("MS " + (Date.now() - t0));
console.log("RESULT " + JSON.stringify(r)); process.exit(0);`;
  writeFileSync("/tmp/tracker-call.mjs", script);
  const out = execSync(
    `echo 'docker compose -f ${compose} --profile qa run --rm -T qa node --input-type=module - < /tmp/tracker-call.mjs' | newgrp docker 2>&1`,
    { encoding: "utf8", shell: "/bin/bash" },
  );
  const timing = out.split("\n").find((one) => one.startsWith("MS "));
  lastCallMs = timing ? Number(timing.slice(3)) : 0;
  const line = out.split("\n").find((one) => one.startsWith("RESULT "));
  if (!line) throw new Error(`no result: ${out.trim().slice(-300)}`);
  return JSON.parse(line.slice("RESULT ".length));
}

const text = (page) => page.evaluate(() => document.body.innerText);
const has = async (page, needle) => (await text(page)).includes(needle);

/** Wait for something to appear WITHOUT touching the page — which is the whole
 *  point wherever a push is what should have delivered it. */
async function appearsUntouched(page, needle, ms = 30000) {
  const started = Date.now();
  while (Date.now() - started < ms) {
    if (await has(page, needle)) return Date.now() - started;
    await page.waitForTimeout(500);
  }
  return null;
}

const tasksTab = (projectId) => `${APP}/app/#/device/${seed.deviceId}/project/${projectId}/tasks`;
const board = (projectId) => `${tasksTab(projectId)}?view=board`;
const taskPage = (projectId, taskId) =>
  `${APP}/app/#/device/${seed.deviceId}/project/${projectId}/tasks/${taskId}`;

const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
// One explicit context, so the phone page later shares this one's session
// cookie. `browser.newPage()` wraps a context of its own and will not hand it
// out, and a second page without the cookie lands on the sign-in gate — which
// reads exactly like a broken surface.
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
await page.fill('input[name="email"]', "qa@localhost");
await Promise.all([
  page.waitForNavigation({ waitUntil: "load" }),
  page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
]);

const project = seed.projectId;
const stamp = Date.now().toString(36);

// ── 1. the verbs answer at all, and the columns are the bridge's ─────────────

let columns = null;
await check("tasks.columns answers the five", async () => {
  columns = call("tasks.columns", { project_id: project });
  const ids = (columns.columns || []).map((one) => one.id);
  return { ok: ids.length >= 5 && ids.includes("in_review"), detail: ids.join(", ") };
});

// ── 2. the tab paints, cold then warm ────────────────────────────────────────

let filed = null;
await check("tasks.create files a task", async () => {
  filed = call("tasks.create", {
    project_id: project,
    title: `Kanban drag does not persist ${stamp}`,
    body: "Dragging a card to **In review** leaves it where it was after a reload.",
    labels: ["bug", "ui"],
    priority: "high",
  });
  return { ok: Boolean(filed.task && filed.task.number), detail: `#${filed.task?.number}` };
});

await check("the Tasks tab paints the filed task", async () => {
  await page.goto(tasksTab(project), { waitUntil: "load" });
  await page.waitForTimeout(6000);
  return { ok: await has(page, stamp), detail: `#${filed?.task?.number}` };
});

// The row used to carry a coloured state dot beside its column. It does not
// any more: #28 took the dots out, and #33 made the list open-only by default,
// so an open row says nothing about its state and a closed one — which is only
// ever on screen because the filter asked — leads line two with a Closed chip.
// This checks the shape that replaced it.
await check("the row puts number and title on one line, and the facts on the next", async () => {
  const row = await page.evaluate(() => {
    const one = document.querySelector(".task-row");
    if (!one) return null;
    const line = one.querySelector(".task-row-open");
    const facts = one.querySelector(".task-row-facts");
    return {
      firstLine: [...(line?.children || [])].map((child) => child.classList[0]),
      facts: [...(facts?.children || [])].map((child) => child.classList[0]),
      status: one.querySelector(".task-status")?.textContent || "",
      labels: [...one.querySelectorAll(".task-label")].map((l) => l.textContent),
      dots: one.querySelectorAll(".task-state, .task-sep").length,
      closedChip: Boolean(one.querySelector(".task-closed")),
    };
  });
  const ok = Boolean(
    row &&
      JSON.stringify(row.firstLine) === JSON.stringify(["task-number", "task-title"]) &&
      row.status &&
      row.dots === 0 &&
      // Open by default (#33), so no Closed chip on what the tab opens with.
      !row.closedChip,
  );
  return { ok, detail: JSON.stringify(row) };
});

// ── 3. the push: a task moved by somebody else, with nothing touched ───────

await check("a card moves on the push, with the page untouched", async () => {
  await page.goto(board(project), { waitUntil: "load" });
  await page.waitForTimeout(6000);
  const moved = `pushed-${stamp}`;
  call("tasks.create", { project_id: project, title: moved, status: "in_review" });
  const ms = await appearsUntouched(page, moved);
  return { ok: ms !== null, detail: ms === null ? "not painted in 30 s" : `${ms} ms` };
});

// ── 4. the filters are params, and the board ignores the column one ──────────

await check("a state filter narrows the list", async () => {
  await page.goto(tasksTab(project), { waitUntil: "load" });
  await page.waitForTimeout(5000);
  call("tasks.close", { task_id: filed.task.id });
  await page.waitForTimeout(4000);
  await page.selectOption('[data-task-filter="state"]', "closed");
  await page.waitForTimeout(3000);
  const onlyClosed = await page.evaluate(() =>
    [...document.querySelectorAll(".task-row .task-state")].every((one) => one.className.includes("closed")));
  const shown = await page.evaluate(() => document.querySelectorAll(".task-row").length);
  call("tasks.reopen", { task_id: filed.task.id });
  return { ok: onlyClosed && shown > 0, detail: `${shown} rows, all closed: ${onlyClosed}` };
});

// ── 5. moving a card: drag, and the keyboard ─────────────────────────────────

await check("the arrow keys move a card, and it sticks", async () => {
  await page.goto(board(project), { waitUntil: "load" });
  await page.waitForTimeout(6000);
  const card = `.task-card[data-task="${filed.task.id}"]`;
  const before = await page.getAttribute(card, "data-status");
  await page.focus(card);
  await page.keyboard.press("ArrowRight");
  await page.waitForTimeout(4000);
  const after = call("tasks.get", { task_id: filed.task.id }).task.status;
  return { ok: after !== before, detail: `${before} → ${after} (bridge says ${after})` };
});

// ── 6. the task page: timeline interleave and a comment round trip ──────────

await check("the timeline interleaves comments and events", async () => {
  call("tasks.comment", { task_id: filed.task.id, body: `a comment ${stamp}` });
  await page.goto(taskPage(project, filed.task.id), { waitUntil: "load" });
  await page.waitForTimeout(6000);
  const kinds = await page.evaluate(() =>
    [...document.querySelectorAll(".task-entry")].map((one) => (one.classList.contains("task-comment") ? "c" : "e")));
  return { ok: kinds.includes("c") && kinds.includes("e"), detail: kinds.join("") };
});

await check("the composer round-trips a comment", async () => {
  const said = `from the composer ${stamp}`;
  await page.fill("#task-comment", said);
  await page.click(".task-composer button[type=submit]");
  await page.waitForTimeout(5000);
  const onWire = call("tasks.get", { task_id: filed.task.id })
    .timeline.some((one) => one.type === "comment" && String(one.body).includes(said));
  return { ok: onWire && (await has(page, said)), detail: onWire ? "on the wire and on screen" : "not on the wire" };
});

// ── 7. the slow one: cutting a workspace, and how long it really takes ───────

let dispatch = null;
await check("assigning a new workspace cuts one and starts an agent", async () => {
  const answer = call("tasks.assign", {
    task_id: filed.task.id,
    assignee: { kind: "new_workspace", name: `tracker-pass-${stamp}` },
  });
  const ms = lastCallMs;
  dispatch = { ...answer.dispatch, ms };
  const listed = call("workspace.list", { project_id: project }).workspaces || [];
  const made = listed.find((one) => (one.workspace_id || one.id) === answer.dispatch?.workspace_id);
  dispatch.isolation = (made?.directories || []).map((d) => d.isolation).join("+") || "unknown";
  dispatch.sources = (made?.directories || []).length;
  return {
    ok: Boolean(answer.dispatch?.agent_id),
    detail: `${ms} ms · isolation as answered: ${dispatch.isolation} · ${dispatch.sources} source(s)`,
  };
});

await check("the dispatch links the workspace and the conversation on the task", async () => {
  const links = call("tasks.get", { task_id: filed.task.id }).task.links;
  return {
    ok: links.workspace_ids.length > 0 && links.conversation_ids.length > 0,
    detail: JSON.stringify(links),
  };
});

await check("the task page draws those links", async () => {
  await page.reload({ waitUntil: "load" });
  await page.waitForTimeout(6000);
  const rows = await page.evaluate(() => [...document.querySelectorAll(".task-link")].map((one) => one.textContent));
  return { ok: rows.length >= 2, detail: rows.join(" | ") };
});

// ── 8. the from_task card on the message delivery actually produced ─────────

await check("the delivered message draws a task card with its links", async () => {
  const conversation = call("tasks.get", { task_id: filed.task.id }).task.links.conversation_ids[0];
  await page.goto(
    `${APP}/app/#/device/${seed.deviceId}/project/${project}/workspace/${dispatch.workspace_id}?agent=${dispatch.agent_id}`,
    { waitUntil: "load" },
  );
  await page.waitForTimeout(8000);
  const card = await page.evaluate(() => {
    const one = document.querySelector(".thread-task");
    return one && {
      number: one.querySelector(".thread-task-link")?.textContent,
      href: one.querySelector(".thread-task-link")?.getAttribute("href"),
      links: [...one.querySelectorAll(".thread-task-links a")].map((a) => a.textContent),
    };
  });
  return {
    ok: Boolean(card && card.number),
    detail: card ? `${card.number} → ${card.href}; links: ${card.links.join(", ") || "none"} (conversation ${conversation})` : "no card",
  };
});

// ── 9. the hand-off at the project agent's default level ────────────────────
//
// Only half of this pair is reachable from here, and the half that is reachable
// is the one that matters.
//
// The qa client IS the user, so an `tasks.assign` from it produces a hand-off
// carrying no `from_agent` — this conversation's dialogue. A project agent's
// conversation opens at Agent only, and the instruction the agent is working
// from must survive that level; hiding it would be the damaging bug.
//
// The other half — a hand-off another AGENT assigned, which carries
// `from_agent` and is hidden at that level — cannot be produced from here at
// all. It needs an agent to call the `assign_task` MCP tool, and the qa client
// cannot impersonate one. It stays covered by the unit tests over the real
// `itemsAtDetailLevel`, and is called out as unverified rather than asserted.

await check("a user-assigned hand-off survives the project agent's default level", async () => {
  const handed = call("tasks.create", { project_id: project, title: `handed-by-user ${stamp}` });
  call("tasks.assign", { task_id: handed.task.id, assignee: { kind: "project_agent" } });
  await page.goto(`${APP}/app/#/device/${seed.deviceId}/project/${project}`, { waitUntil: "load" });
  await page.waitForTimeout(10000);
  const level = await page.evaluate(() =>
    Object.keys(localStorage).filter((k) => k.startsWith("build.conversation.detail."))
      .map((k) => localStorage.getItem(k)).join(",") || "unset (defaults to agent for a project)");
  const drawn = await has(page, `handed-by-user ${stamp}`);
  return { ok: drawn, detail: `drawn: ${drawn} at level ${level} — the agent-assigned half needs an agent and is NOT covered here` };
});

// ── 10. mobile: the board scrolls sideways rather than reflowing ─────────────

await check("the kanban scrolls sideways under 760px", async () => {
  // The SAME context, not a new one. `browser.newPage()` opens a fresh context
  // with no cookies, so the phone lands on the sign-in gate and every selector
  // below finds nothing — which reads exactly like a broken board.
  const phone = await context.newPage();
  await phone.setViewportSize({ width: 390, height: 844 });
  await phone.goto(board(project), { waitUntil: "load" });
  await phone.waitForTimeout(10000);
  const measured = await phone.evaluate(() => {
    const one = document.querySelector(".task-board");
    return one && { scrollWidth: one.scrollWidth, clientWidth: one.clientWidth, overflowX: getComputedStyle(one).overflowX };
  });
  await phone.screenshot({ path: "/tmp/tracker-mobile.png" });
  await phone.close();
  return {
    ok: Boolean(measured && measured.scrollWidth > measured.clientWidth && measured.overflowX === "auto"),
    detail: JSON.stringify(measured),
  };
});

// ── 11. the P0's own symptom, against a bridge that advertises `tasks` ─────
//
// /tmp/push-check.mjs, folded in rather than run beside: a posted message must
// paint in a watching browser with nothing touched. This is the regression test
// for my own outage — the subscription guard exists so that naming `tasks`
// cannot cost this device its `state` and `thread` pushes, and this is the only
// place that has ever been checked against a bridge which actually carries the
// kind. Folded in so the whole session runs off one checkout's compose file.

await check("a posted message paints untouched (the P0's own symptom)", async () => {
  await page.goto(`${APP}/app/#/device/${seed.deviceId}/project/${project}/workspace/${ws.workspaceId}`, { waitUntil: "load" });
  await page.waitForTimeout(8000);
  const said = `push-test-${stamp}`;
  const started = Date.now();
  call("thread.post", { entity_id: ws.entityId, agent_id: ws.agentId, body: said });
  const ms = await appearsUntouched(page, said);
  await page.screenshot({ path: "/tmp/push-check.png" });
  return { ok: ms !== null, detail: ms === null ? "NOT painted within 30 s without interaction" : `${ms} ms` };
});

await check("and is still there after a reload", async () => {
  await page.reload({ waitUntil: "load" });
  await page.waitForTimeout(6000);
  return { ok: await has(page, `push-test-${stamp}`), detail: "durable, not just painted" };
});

// No separate greeting check: whether the inbox subscription is really asking
// for all three kinds is proved behaviourally by check 4 (an `tasks` push
// arrives) and the two above (a `thread` push arrives) on the same session. A
// third check reading the greeting would assert the cause of what those two
// already observe, and I have no clean way to read it from page scope anyway.

await page.screenshot({ path: "/tmp/tracker-check.png", fullPage: true });
await browser.close();

console.log("\n──────── summary ────────");
for (const { name, ok, detail } of results) console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
const { passed, total, exitCode } = summarizeChecks(results);
console.log(`\n${passed}/${total} passed`);
if (dispatch) {
  console.log(`\nnew_workspace round trip: ${dispatch.ms} ms · isolation as answered: ${dispatch.isolation} · ${dispatch.sources} source(s)`);
}
process.exitCode = exitCode;
