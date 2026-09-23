// #119's browser pass: an issue moved to Done over MCP while the SPA was cut
// off from a bridge that restarted must leave both "Needs you" surfaces once
// the SPA is back — the Issues list's group and the Dashboard's tab.
//
//   ISSUES_REPO=<a build-web checkout> COMPOSE_PROJECT=needsyou119 \
//     APP_URL=http://localhost:8090 node web/needs-you-check.mjs
//
// Real bridge, real MCP, real SPA: nothing here is stubbed. It reads the seed
// web/live-seed.mjs printed (SEED_FILE, default /tmp/live-seed.json) and
// exits 0 only when both surfaces dropped the issue. Every wait is a
// condition with a deadline, never a fixed sleep standing in for one — except
// the quiet hold that proves the gap: the move must NOT have reached the page
// while it was offline, or this run tested nothing.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { execSync } from "node:child_process";
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
const agentId = seed.workspaces[0].agentId;

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

/** One MCP tool call, the way an agent makes it: `build-bridge mcp --task`
 *  over stdio inside the bridge container, talking to the running daemon. */
function mcpTool(name, args) {
  const frames = [
    { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "needs-you-check", version: "1" } } },
    { jsonrpc: "2.0", method: "notifications/initialized" },
    { jsonrpc: "2.0", id: 2, method: "tools/call", params: { name, arguments: args } },
  ].map((frame) => JSON.stringify(frame)).join("\n");
  writeFileSync("/tmp/119-mcp.jsonl", `${frames}\n`);
  const out = docker(`${compose} exec -T bridge build-bridge mcp --task ${agentId} < /tmp/119-mcp.jsonl`);
  const reply = out.split("\n").map((one) => { try { return JSON.parse(one); } catch { return null; } })
    .find((frame) => frame?.id === 2);
  if (!reply || reply.error || reply.result?.isError) throw new Error(`mcp ${name}: ${out.trim().slice(-400)}`);
  return reply.result;
}

async function until(what, test, ms = 60000) {
  const started = Date.now();
  while (Date.now() - started < ms) {
    if (await test()) return Date.now() - started;
    await new Promise((done) => setTimeout(done, 250));
  }
  throw new Error(`timed out after ${ms} ms waiting for ${what}`);
}

const stamp = `needs-you ${Date.now().toString(36)}`;
const filed = call("issues.create", { project_id: seed.projectId, title: stamp, body: "#119 check" });
const issueId = filed.issue.id;
call("issues.update", { issue_id: issueId, status: "in_review" });
console.log(`filed #${filed.issue.number} ${issueId}, in review`);

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

const listNeedsYou = (page) => page.evaluate((title) => [...document.querySelectorAll('[data-issue-group="needsYou"] .issue-title')]
  .some((one) => one.textContent.includes(title)), stamp);
const dashNeedsYou = (page) => page.evaluate((title) => {
  const count = Number(document.querySelector('[data-dashboard-tab="needsYou"] .issue-dashboard-count')?.textContent ?? NaN);
  const listed = [...document.querySelectorAll('[data-dashboard-section="needsYou"] .issue-dashboard-title, [role="tabpanel"] .issue-dashboard-title')]
    .some((one) => one.textContent.includes(title));
  return { count, listed };
}, stamp);

await until("the list's Needs you to show the in-review issue", () => listNeedsYou(list));
await until("the Dashboard's Needs you to show it", async () => (await dashNeedsYou(dash)).listed);
const countBefore = (await dashNeedsYou(dash)).count;
await list.screenshot({ path: `${SHOTS}/${LABEL}-1-in-review.png` });
console.log(`cached: both surfaces list it; dashboard Needs you = ${countBefore}`);

// Cut the SPA off, restart the bridge under it (its revision counter starts
// again at zero), and move the issue to Done over MCP while nobody listens.
await context.setOffline(true);
docker(`${compose} restart bridge`);
await until("the restarted bridge to take an MCP call", async () => {
  try {
    mcpTool("list_issues", {});
    return true;
  } catch {
    return false;
  }
}, 90000);
mcpTool("move_issue", { issue_id: issueId, status: "done" });
console.log("moved to done over MCP while the SPA was offline");
await new Promise((done) => setTimeout(done, 3000));
const gapHeld = (await listNeedsYou(list)) && (await dashNeedsYou(dash)).listed;
if (!gapHeld) {
  console.log("FAIL  the move reached the page while it was offline — this run did not test the gap");
  await browser.close();
  process.exit(3);
}

await context.setOffline(false);
let failed = false;
for (const [name, gone] of [
  ["the Issues list's Needs you group drops it", async () => !(await listNeedsYou(list))],
  ["the Dashboard's Needs you drops it and its count falls by one", async () => {
    const now = await dashNeedsYou(dash);
    return !now.listed && now.count === countBefore - 1;
  }],
]) {
  try {
    const ms = await until(name, gone, 60000);
    console.log(`PASS  ${name} — ${ms} ms after reconnect`);
  } catch (error) {
    failed = true;
    console.log(`FAIL  ${name} — ${error.message}`);
  }
}
await list.screenshot({ path: `${SHOTS}/${LABEL}-2-after-reconnect-list.png` });
await dash.screenshot({ path: `${SHOTS}/${LABEL}-3-after-reconnect-dashboard.png` });
await browser.close();
process.exit(failed ? 1 : 0);
