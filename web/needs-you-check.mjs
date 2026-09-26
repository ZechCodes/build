// #119's browser pass: an issue moved to Done must leave both "Needs you"
// surfaces — the Issues list's group and the Dashboard's tab — in the ways a
// move can go unheard:
//
//   churn      the project keeps pushing (comments every 100 ms) while every
//              `issues.list` round trip is slower than the bridge's 250 ms
//              flush. Before #119 each push started a read that the next one
//              overtook, so no answer landed until the churn stopped.
//   reconnect  the SPA is offline while the bridge restarts and the move is
//              made; the greeting's refetch must bring it in. Run after
//              churn, this is #123's churn → offline → restart → online.
//              Offline only once no page holds a relay socket, and each page
//              must dial a fresh session after it is back online (#130).
//   killed     the bridge is SIGKILLed while the page is online and comes
//              straight back (#123). No close reaches the page, so its ICE
//              restart reaches the NEW process, which answers it: the restart
//              must prove it carries the session (or be torn down for a fresh
//              one) and must greet it again. Also checks the status ring
//              stopped saying connected, and screenshots it when it did.
//
//   ISSUES_REPO=<a build-web checkout> COMPOSE_PROJECT=needsyou119 \
//     APP_URL=http://localhost:8090 [SCENARIOS=churn,reconnect,killed] [RUNS=5] \
//     node web/needs-you-check.mjs
//
// RUNS repeats the scenario list in the same browser, so each round starts
// from wherever the last one left the page. A failed check writes each page's
// connection diagnostics next to the screenshots (DUMP_ALWAYS=1: every round);
// CONSOLE=1 echoes the pages' consoles.
//
// Real bridge, real SPA: nothing is stubbed. Moves are made by a second
// client (`issues.update`), the same tracker write and `issues` push note an
// agent's MCP `move_issue` makes — the compose bridge's harness is `cat`,
// which holds no MCP session token, so its daemon socket refuses tool calls.
//
// `churn` needs a slow list: it seeds 150 issues of ~30 KB (once per stack)
// and adds egress delay (NETEM_DELAY, default 100ms) on the bridge container's eth0 with
// `tc netem`, from a throwaway NET_ADMIN container in that network namespace,
// removed again at the end. Use it on your OWN compose project only.
//
// Reads the seed web/live-seed.mjs printed (SEED_FILE, default
// /tmp/live-seed.json); exits 0 only when every scenario passed. Waits are
// conditions with deadlines, except the holds that prove the move had NOT
// reached the page yet — without them a run could pass having tested nothing.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { execSync, spawn } from "node:child_process";
import { chromium } from "playwright";
import { freshDials, goOfflineWithNoOpenSocket, goOnline, trackWebSockets } from "./offlineEvidence.mjs";

const APP = process.env.APP_URL || "http://localhost:8090";
const REPO = process.env.ISSUES_REPO;
const PROJECT = process.env.COMPOSE_PROJECT;
if (!REPO || !PROJECT) {
  console.error("set ISSUES_REPO and COMPOSE_PROJECT — this run must not guess which stack it is on");
  process.exit(2);
}
const SHOTS = process.env.SHOTS_DIR || "/tmp/119-shots";
const LABEL = process.env.SHOT_LABEL || "run";
mkdirSync(SHOTS, { recursive: true });
const compose = `docker compose -p ${PROJECT} -f ${REPO}/deploy/compose.real.yml`;
const seed = JSON.parse(readFileSync(process.env.SEED_FILE || "/tmp/live-seed.json", "utf8"));

const docker = (command) =>
  execSync(`echo ${JSON.stringify(command)} | newgrp docker 2>&1`, { encoding: "utf8", shell: "/bin/bash" });

/** One bridge RPC from inside the qa container, as web/tracker-check.mjs does. */
function call(method, params) {
  const script = `import * as transport from "@build/secure-transport"; import { openDeviceLink, openRendezvous } from "./client.mjs"; import { loginWithDummy } from "./skrift-auth.mjs";
const { cookie, mintGatewayToken } = await loginWithDummy(process.env.API_URL, { email: "qa@localhost" });
const rendezvous = await openRendezvous({ relayUrl: process.env.RELAY_URL, mintGatewayToken });
const link = await openDeviceLink({ rendezvous, transport, apiUrl: process.env.API_URL, cookie });
const r = await link.session.call(${JSON.stringify(method)}, ${JSON.stringify(params)});
console.log("RESULT " + JSON.stringify(r)); process.exit(0);`;
  writeFileSync("/tmp/119-call.mjs", script);
  const out = docker(`${compose} --profile qa run --rm --no-deps -T qa node --input-type=module - < /tmp/119-call.mjs`);
  const line = out.split("\n").find((one) => one.startsWith("RESULT "));
  if (!line) throw new Error(`${method}: no result: ${out.trim().slice(-300)}`);
  return JSON.parse(line.slice("RESULT ".length));
}

async function until(what, test, ms = 60000) {
  const started = Date.now();
  while (Date.now() - started < ms) {
    if (await test()) return Date.now() - started;
    await new Promise((done) => setTimeout(done, 250));
  }
  throw new Error(`timed out after ${ms} ms waiting for ${what}`);
}


const SCENARIOS = (process.env.SCENARIOS || "churn,reconnect,killed").split(",");
const RUNS = Number(process.env.RUNS || 1);
let round = 1;
const run = Date.now().toString(36);

/** One qa script over one device link, run in the background. */
function qaScript(name, source) {
  writeFileSync(`/tmp/119-${name}.mjs`, `import * as transport from "@build/secure-transport"; import { openDeviceLink, openRendezvous } from "./client.mjs"; import { loginWithDummy } from "./skrift-auth.mjs";
const { cookie, mintGatewayToken } = await loginWithDummy(process.env.API_URL, { email: "qa@localhost" });
const rendezvous = await openRendezvous({ relayUrl: process.env.RELAY_URL, mintGatewayToken });
const link = await openDeviceLink({ rendezvous, transport, apiUrl: process.env.API_URL, cookie });
${source}
process.exit(0);`);
  const child = spawn("/bin/bash", ["-c", `echo '${compose} --profile qa run --rm --no-deps -T qa node --input-type=module - < /tmp/119-${name}.mjs' | newgrp docker`]);
  const lines = [];
  child.stdout.on("data", (data) => lines.push(...String(data).split("\n")));
  child.stderr.on("data", (data) => lines.push(...String(data).split("\n")));
  return { lines, done: new Promise((resolve) => child.on("exit", resolve)) };
}

const DELAY = process.env.NETEM_DELAY || "100ms";
const netem = (verb) => docker(`docker run --rm --net container:${PROJECT}-bridge-1 --cap-add NET_ADMIN alpine sh -c "apk add -q iproute2 >/dev/null 2>&1; tc qdisc ${verb} dev eth0 root ${verb === "del" ? "" : `netem delay ${DELAY}`}"`);

const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
const context = await browser.newContext({ viewport: { width: 1280, height: 860 } });
// Every WebSocket each page opens, and whether it is still open: the
// reconnect scenario's offline precondition (web/offlineEvidence.mjs, #130).
await context.addInitScript(trackWebSockets);
const login = await context.newPage();
await login.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
await login.fill('input[name="email"]', "qa@localhost");
await Promise.all([
  login.waitForNavigation({ waitUntil: "load" }),
  login.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
]);
await login.close();

const tab = (view) => `${APP}/app/#/device/${seed.deviceId}/project/${seed.projectId}/issues?view=${view}`;
const list = await context.newPage();
await list.goto(tab("list"), { waitUntil: "load" });
const dash = await context.newPage();
await dash.goto(tab("dashboard"), { waitUntil: "load" });
const t0 = Date.now();
for (const [name, page] of [["list", list], ["dash", dash]]) {
  page.on("console", (message) => {
    if (process.env.CONSOLE) console.log(`[${name} ${((Date.now() - t0) / 1000).toFixed(1)}] ${message.text()}`);
  });
}

/** Each page's connection record, written out when a scenario fails: what
 *  it dialled, when, and why — the evidence a stalled reconnect leaves. */
async function dumpDiagnostics(label) {
  for (const [name, page] of [["list", list], ["dash", dash]]) {
    const report = await page.evaluate(() => globalThis.buildConnectionDiagnostics?.()).catch((error) => ({ error: error.message }));
    writeFileSync(`${SHOTS}/${LABEL}-r${round}-${label}-${name}-diagnostics.json`, JSON.stringify(report, null, 1));
  }
}

const listNeedsYou = (title) => list.evaluate((wanted) => [...document.querySelectorAll('[data-issue-group="needsYou"] .issue-title')]
  .some((one) => one.textContent.includes(wanted)), title);
/** Whether the list's Needs you group is a whole paint: its header count
 *  matches the rows under it. "Dropped" then means the group repainted
 *  without the issue — not a list caught empty mid-repaint. */
const listNeedsYouSettled = () => list.evaluate(() => {
  const group = document.querySelector('[data-issue-group="needsYou"]');
  const count = Number(group?.querySelector(".issue-group-count")?.textContent ?? NaN);
  return count === group?.querySelectorAll(".issue-title").length;
});
const dashNeedsYou = (title) => dash.evaluate((wanted) => ({
  count: Number(document.querySelector('[data-dashboard-tab="needsYou"] .issue-dashboard-count')?.textContent ?? NaN),
  listed: [...document.querySelectorAll('[role="tabpanel"] .issue-dashboard-title')].some((one) => one.textContent.includes(wanted)),
}), title);

let failed = false;
const report = (ok, name, detail = "") => {
  if (!ok) failed = true;
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
};

/** File an issue in review and wait until both surfaces say it needs you. */
async function inReview(label) {
  const title = `${label} ${run}`;
  const { issue } = call("issues.create", { project_id: seed.projectId, title, body: "#119 check" });
  call("issues.update", { issue_id: issue.id, status: "in_review" });
  await until(`${title} under the list's Needs you`, () => listNeedsYou(title), 180000);
  await until(`${title} under the Dashboard's Needs you`, async () => (await dashNeedsYou(title)).listed, 180000);
  return { id: issue.id, title, countBefore: (await dashNeedsYou(title)).count };
}

/** Both surfaces drop it, the Dashboard's count by one, within `ms` — each
 *  timed from the same moment, not one after the other. */
async function bothDrop(scenario, target, ms) {
  const dropped = await Promise.all([
    ["the Issues list's Needs you group drops it", async () =>
      !(await listNeedsYou(target.title)) && (await listNeedsYouSettled())],
    ["the Dashboard's Needs you drops it and its count falls by one", async () => {
      const now = await dashNeedsYou(target.title);
      return !now.listed && now.count === target.countBefore - 1;
    }],
  ].map(async ([name, gone]) => {
    try {
      report(true, `${scenario}: ${name}`, `${await until(name, gone, ms)} ms`);
      return true;
    } catch (error) {
      report(false, `${scenario}: ${name}`, error.message);
      return false;
    }
  }));
  return dropped.every(Boolean);
}

async function churn() {
  // Counted inside the container: the whole list is megabytes, far past one
  // line of piped output.
  const counting = qaScript("count", `const r = await link.session.call("issues.list", { project_id: ${JSON.stringify(seed.projectId)} });
console.log("BULK " + r.issues.filter((one) => one.title.startsWith("bulk ")).length);`);
  await counting.done;
  const bulk = Number((counting.lines.find((line) => line.startsWith("BULK ")) || "BULK 0").slice(5));
  if (bulk < 150) {
    const seeding = qaScript("bulk", `const body = "bulk ".repeat(6000);
for (let i = 0; i < 150; i++) await link.session.call("issues.create", { project_id: ${JSON.stringify(seed.projectId)}, title: "bulk " + i, body });`);
    await seeding.done;
  }
  const busy = call("issues.create", { project_id: seed.projectId, title: `busy ${run}`, body: "churn target" }).issue.id;
  const target = await inReview("churn");
  netem("add");
  try {
    // Comments every 100 ms, fired without waiting for each answer, and the
    // move three seconds in: the move's push lands mid-churn.
    const churning = qaScript("churn", `const end = Date.now() + 45000; let n = 0; let moved = false; const t0 = Date.now();
while (Date.now() < end) {
  if (!moved && Date.now() - t0 > 3000) { await link.session.call("issues.update", { issue_id: ${JSON.stringify(target.id)}, status: "done" }); moved = true; console.log("MOVED"); }
  link.session.call("issues.comment", { issue_id: ${JSON.stringify(busy)}, body: "tick " + (n++) }).catch(() => {});
  await new Promise((r) => setTimeout(r, 100));
}
console.log("CHURN_END " + n);`);
    await until("the move mid-churn", async () => churning.lines.includes("MOVED"), 90000);
    // Forty-two seconds of churn still to go after the move, and each list
    // read takes seconds on this link: both surfaces must drop the issue
    // within thirty-five, while the pushes keep coming — not once they stop.
    if (await bothDrop("churn", target, 35000))
      report(!churning.lines.some((line) => line.startsWith("CHURN_END")), "churn: still churning when both had dropped it");
    await list.screenshot({ path: `${SHOTS}/${LABEL}-churn-list.png` });
    await dash.screenshot({ path: `${SHOTS}/${LABEL}-churn-dashboard.png` });
    await churning.done;
  } finally {
    netem("del");
  }
}

async function reconnect() {
  const target = await inReview("reconnect");
  // A socket still open would carry signalling through the "offline" page.
  // The SPA closes each once nothing negotiates on it; checked again once
  // offline has applied, so it holds when the bridge restarts.
  await goOfflineWithNoOpenSocket(context, [list, dash]);
  docker(`${compose} restart bridge`);
  await until("the restarted bridge to answer", async () => {
    try {
      call("issues.get", { issue_id: target.id });
      return true;
    } catch {
      return false;
    }
  }, 90000);
  call("issues.update", { issue_id: target.id, status: "done" });
  await new Promise((done) => setTimeout(done, 3000));
  const held = (await listNeedsYou(target.title)) && (await dashNeedsYou(target.title)).listed;
  report(held, "reconnect: the move had not reached the offline page (the gap is real)");
  if (!held) await dumpDiagnostics("reconnect-held");
  // Each page's clock read before networking is back: a fresh dial can land
  // before anything here could ask.
  const onlineAt = await goOnline(context, [list, dash]);
  console.log(`[${((Date.now() - t0) / 1000).toFixed(1)}] ONLINE`);
  // The SPA's own reconnect backs off while it was offline, so the deadline
  // covers that backoff too; the time is from going back online.
  let dropped = await bothDrop("reconnect", target, 180000);
  // The restarted bridge holds none of the old sessions, so what brought the
  // move in must be a session to the seeded machine, dialled after the page
  // came back.
  for (const [index, [name, page]] of [["list", list], ["dash", dash]].entries()) {
    const dials = await freshDials(page, onlineAt[index], seed.deviceId);
    report(dials.length > 0, `reconnect: the ${name} page dialled a fresh session once online`,
      dials.map((entry) => entry.connection).join(", "));
    dropped &&= dials.length > 0;
  }
  if (!dropped) await dumpDiagnostics("reconnect");
  await list.screenshot({ path: `${SHOTS}/${LABEL}-reconnect-list.png` });
}

/** The bridge dies without a word — SIGKILL, so no SCTP abort or DTLS close
 *  reaches the page — and comes straight back while the page is online. The
 *  page's ICE restart then reaches the NEW process, which answers it; before
 *  #123 the channels still read `open` from the dead association, so the
 *  restart "landed" every six seconds for ever and no session was minted. */
async function killed() {
  const target = await inReview("killed");
  // What the status ring says from here on: an app that has no session must
  // say so while it has none, not keep showing connected.
  // Read every 100 ms rather than observed: the ring's button can be
  // rebuilt, and an observer on the old one hears nothing after that.
  await list.evaluate(() => {
    const started = Date.now();
    const read = () => document.querySelector(".connection-status")?.dataset.state;
    clearInterval(globalThis.__ringWatch);
    globalThis.__ring = [[0, read()]];
    globalThis.__ringWatch = setInterval(() => {
      const state = read();
      if (state !== globalThis.__ring.at(-1)[1]) globalThis.__ring.push([Date.now() - started, state]);
    }, 100);
  });
  let shotAway = false;
  const watchRing = setInterval(async () => {
    if (shotAway) return;
    const state = await list.evaluate(() => document.querySelector(".connection-status")?.dataset.state).catch(() => null);
    if (state && state !== "connected") {
      shotAway = true;
      // Once the ring has finished turning amber: a shot mid-transition shows
      // the colour it is leaving.
      await list.waitForFunction(() => {
        const ring = document.querySelector(".connection-status");
        const probe = document.body.appendChild(Object.assign(document.createElement("i"), { style: "color:var(--amber)" }));
        const amber = getComputedStyle(probe).color;
        probe.remove();
        return ring && getComputedStyle(ring, "::before").borderTopColor === amber;
      }, null, { timeout: 3000 }).catch(() => {});
      await list.screenshot({ path: `${SHOTS}/${LABEL}-r${round}-killed-ring-${state}.png` }).catch(() => {});
    }
  }, 250);
  const killedAt = Date.now();
  docker(`${compose} kill -s KILL bridge`);
  docker(`${compose} up -d --no-deps bridge`);
  await until("the restarted bridge to answer", async () => {
    try {
      call("issues.get", { issue_id: target.id });
      return true;
    } catch {
      return false;
    }
  }, 90000);
  call("issues.update", { issue_id: target.id, status: "done" });
  console.log(`[${((Date.now() - t0) / 1000).toFixed(1)}] MOVED ${((Date.now() - killedAt) / 1000).toFixed(1)} s after the kill`);
  const dropped = await bothDrop("killed", target, 60000);
  clearInterval(watchRing);
  const ring = await list.evaluate(() => globalThis.__ring);
  console.log(`      ring after the kill: ${ring.map(([at, state]) => `${(at / 1000).toFixed(1)}s ${state}`).join(" → ")}`);
  report(ring.some(([, state]) => state !== "connected"), "killed: the ring stopped saying connected while there was no session");
  if (!dropped || process.env.DUMP_ALWAYS) await dumpDiagnostics("killed");
  await list.screenshot({ path: `${SHOTS}/${LABEL}-killed-list.png` });
}

for (round = 1; round <= RUNS; round++) {
  if (RUNS > 1) console.log(`== round ${round} of ${RUNS}`);
  for (const scenario of SCENARIOS) {
    try {
      await { churn, reconnect, killed }[scenario]();
    } catch (error) {
      report(false, scenario, error.message);
      await dumpDiagnostics(scenario);
    }
  }
}
await browser.close();
process.exit(failed ? 1 : 0);
