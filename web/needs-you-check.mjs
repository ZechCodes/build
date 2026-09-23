// #119's browser pass: an issue moved to Done must leave both "Needs you"
// surfaces — the Issues list's group and the Dashboard's tab — in the two
// ways a move can go unheard:
//
//   churn      the project keeps pushing (comments every 100 ms) while every
//              `issues.list` round trip is slower than the bridge's 250 ms
//              flush. Before #119 each push started a read that the next one
//              overtook, so no answer landed until the churn stopped.
//   reconnect  the SPA is offline while the bridge restarts and the move is
//              made; the greeting's refetch must bring it in.
//
//   ISSUES_REPO=<a build-web checkout> COMPOSE_PROJECT=needsyou119 \
//     APP_URL=http://localhost:8090 [SCENARIOS=churn,reconnect] node web/needs-you-check.mjs
//
// Real bridge, real SPA: nothing is stubbed. Moves are made by a second
// client (`issues.update`), the same tracker write and `issues` push note an
// agent's MCP `move_issue` makes — the compose bridge's harness is `cat`,
// which holds no MCP session token, so its daemon socket refuses tool calls.
//
// `churn` needs a slow list: it seeds 150 issues of ~30 KB (once per stack)
// and adds 250 ms of egress delay on the bridge container's eth0 with
// `tc netem`, from a throwaway NET_ADMIN container in that network namespace,
// removed again at the end. Use it on your OWN compose project only.
//
// Reads the seed web/live-seed.mjs printed (SEED_FILE, default
// /tmp/live-seed.json); exits 0 only when every scenario passed. Waits are
// conditions with deadlines, except the holds that prove the move had NOT
// reached the page yet — without them a run could pass having tested nothing.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { execSync, spawn } from "node:child_process";
import { chromium } from "/home/zech/.local/share/mise/installs/npm-playwright/1.63.0/node_modules/playwright/index.mjs";

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


const SCENARIOS = (process.env.SCENARIOS || "churn,reconnect").split(",");
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

const netem = (verb) => docker(`docker run --rm --net container:${PROJECT}-bridge-1 --cap-add NET_ADMIN alpine sh -c "apk add -q iproute2 >/dev/null 2>&1; tc qdisc ${verb} dev eth0 root ${verb === "del" ? "" : "netem delay 250ms"}"`);

const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
const context = await browser.newContext({ viewport: { width: 1280, height: 860 } });
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

const listNeedsYou = (title) => list.evaluate((wanted) => [...document.querySelectorAll('[data-issue-group="needsYou"] .issue-title')]
  .some((one) => one.textContent.includes(wanted)), title);
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

/** Both surfaces drop it, the Dashboard's count by one, within `ms`. */
async function bothDrop(scenario, target, ms) {
  for (const [name, gone] of [
    ["the Issues list's Needs you group drops it", async () => !(await listNeedsYou(target.title))],
    ["the Dashboard's Needs you drops it and its count falls by one", async () => {
      const now = await dashNeedsYou(target.title);
      return !now.listed && now.count === target.countBefore - 1;
    }],
  ]) {
    try {
      report(true, `${scenario}: ${name}`, `${await until(name, gone, ms)} ms`);
    } catch (error) {
      report(false, `${scenario}: ${name}`, error.message);
    }
  }
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
    // read takes seconds on this link: the page must drop the issue within
    // thirty, while the pushes keep coming — not only once they stop.
    await bothDrop("churn", target, 30000);
    await list.screenshot({ path: `${SHOTS}/${LABEL}-churn-list.png` });
    await dash.screenshot({ path: `${SHOTS}/${LABEL}-churn-dashboard.png` });
    await churning.done;
  } finally {
    netem("del");
  }
}

async function reconnect() {
  const target = await inReview("reconnect");
  await context.setOffline(true);
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
  await context.setOffline(false);
  await bothDrop("reconnect", target, 60000);
  await list.screenshot({ path: `${SHOTS}/${LABEL}-reconnect-list.png` });
}

for (const scenario of SCENARIOS) {
  try {
    await { churn, reconnect }[scenario]();
  } catch (error) {
    report(false, scenario, error.message);
  }
}
await browser.close();
process.exit(failed ? 1 : 0);
