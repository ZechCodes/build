// Browser push, end to end (#191): a real Chromium turns notifications on from
// the inbox rail's prompt, and a real bridge's notify travels through the local
// api and the browser's push service to the service worker, which shows it.
//
// What it proves, each against the running compose stack:
//   1. the rail offers "Turn on"; the click asks for permission and the
//      subscription reaches the api, and the offer is gone after;
//   2. the user's own comment on a watched task pushes nothing;
//   3. an agent answering (a Complete report) pushes "An agent needs you", and
//      its comment on the watched task pushes "New activity on a task" — both
//      sent by the bridge's own triggers; each deep link opens its surface.
//
// The agent is web/push-harness/claude, a stand-in harness that acts through
// the bridge's real MCP entry (the scripted QA agent never speaks). Usage
// (host), with a stack started from deploy/compose.real.yml layered with
// deploy/compose.push-check.yml, BUILD_VAPID_* set, and paired (pair.mjs):
//   COMPOSE="docker compose -p <project> -f <checkout>/deploy/compose.real.yml -f <checkout>/deploy/compose.push-check.yml" \
//   BRIDGE_CONTAINER=<project>-bridge-1 VAPID_ENV=<file of BUILD_VAPID_*=…> \
//   PUSH_SEED=<checkout>/web/push-seed.mjs node web/push-check.mjs
//
// The browser's own "Allow" is the one thing a headless run cannot click, so
// the page's permission request grants through Playwright first — the prompt,
// the subscription and everything after are the app's own.

import { execSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright";

const APP = process.env.APP || "http://localhost:8090";
const EMAIL = process.env.QA_EMAIL || "qa@localhost";
const COMPOSE = process.env.COMPOSE;
const BRIDGE = process.env.BRIDGE_CONTAINER;
const VAPID_ENV = process.env.VAPID_ENV;
const PUSH_SEED = process.env.PUSH_SEED;
const OUT = process.env.OUT || "/tmp/push-check";
if (!COMPOSE || !BRIDGE || !VAPID_ENV || !PUSH_SEED) throw new Error("set COMPOSE, BRIDGE_CONTAINER, VAPID_ENV and PUSH_SEED");
execSync(`mkdir -p ${OUT}`);

const scratch = mkdtempSync(join(tmpdir(), "push-check-"));
/** Run a shell script with the docker group, as `newgrp docker` gives it. */
const docker = (script) => {
  const file = join(scratch, `step-${Date.now()}.sh`);
  writeFileSync(file, `set -e\nset -a; . ${VAPID_ENV}; set +a\n${script}\n`);
  return execSync(`echo "sh ${file}" | newgrp docker`, { encoding: "utf8", stdio: ["pipe", "pipe", "inherit"] });
};
/** One push-seed.mjs step inside the qa container, without touching the stack. */
const seedStep = (...args) => {
  const quoted = args.map((arg) => `'${arg}'`).join(" ");
  const out = docker(`${COMPOSE} --profile qa run --rm --no-deps -T -v ${PUSH_SEED}:/app/push-seed.mjs:ro qa node push-seed.mjs ${quoted} </dev/null`);
  const line = out.split("\n").find((row) => row.startsWith("PUSH "));
  if (!line) throw new Error(`push-seed ${args[0]} said nothing:\n${out}`);
  return JSON.parse(line.slice(5));
};
/** Name the task the stand-in harness comments on, before its turn starts. */
const tellTheHarness = (taskId) => {
  const file = join(scratch, "push-task");
  writeFileSync(file, taskId);
  docker(`docker cp ${file} ${BRIDGE}:/tmp/push-task`);
};

const results = [];
const note = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "✓" : "✗"} ${name}${detail ? `  — ${detail}` : ""}`);
};

const seeded = seedStep("seed");
console.log(`seeded ${JSON.stringify(seeded)}`);

const context = await chromium.launchPersistentContext(mkdtempSync(join(tmpdir(), "push-profile-")), {
  executablePath: process.env.CHROMIUM_PATH || "/usr/bin/chromium",
  headless: true,
  viewport: { width: 1280, height: 860 },
});
await context.exposeBinding("__allowNotifications", async () => context.grantPermissions(["notifications"], { origin: APP }));
await context.addInitScript(() => {
  if (typeof Notification === "undefined") return;
  const ask = Notification.requestPermission.bind(Notification);
  Notification.requestPermission = async () => {
    await window.__allowNotifications();
    return ask();
  };
});
const page = context.pages()[0] || (await context.newPage());

const notifications = () => page.evaluate(async () => {
  const registration = await navigator.serviceWorker.getRegistration("/app/");
  const shown = registration ? await registration.getNotifications() : [];
  return shown.map((n) => ({ title: n.title, body: n.body, tag: n.tag, url: n.data?.url }));
});
const clearNotifications = () => page.evaluate(async () => {
  const registration = await navigator.serviceWorker.getRegistration("/app/");
  for (const n of registration ? await registration.getNotifications() : []) n.close();
});
const waitForNotification = async (tag, budgetMs) => {
  const t0 = Date.now();
  while (Date.now() - t0 < budgetMs) {
    const found = (await notifications()).find((n) => n.tag === tag);
    if (found) return { ...found, ms: Date.now() - t0 };
    await page.waitForTimeout(250);
  }
  return null;
};

try {
  await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
  await page.fill('input[name="email"]', EMAIL);
  await page.fill('input[name="name"]', "Push Check");
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load", timeout: 15000 }).catch(() => {}),
    page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
  ]);
  await page.goto(`${APP}/app/`, { waitUntil: "load" });

  // ---- 1. the offer, the click, the subscription ----
  const offer = page.locator("#push-prompt [data-push-enable]");
  await offer.waitFor({ state: "visible", timeout: 60000 });
  await page.screenshot({ path: `${OUT}/1-offer.png` });
  note("the inbox rail offers to turn notifications on", true, await page.locator("#push-prompt").innerText());
  await offer.click();
  const storedSubscriptions = () => Number(docker(`${COMPOSE} exec -T app /app/.venv/bin/python -c "import sqlite3; print(sqlite3.connect('/app/app.db').execute('select count(*) from push_subscriptions').fetchone()[0])" </dev/null`).trim());
  const subscribedAt = Date.now();
  let subscribed = null;
  while (!subscribed && Date.now() - subscribedAt < 60000) {
    subscribed = await page.evaluate(async () => {
      const registration = await navigator.serviceWorker.getRegistration("/app/");
      return (await registration?.pushManager.getSubscription())?.endpoint || null;
    });
    if (!subscribed) await page.waitForTimeout(250);
  }
  note("the click subscribes this browser with the push service", Boolean(subscribed), subscribed ? `${new URL(subscribed).host} after ${Date.now() - subscribedAt} ms` : "none in 60 s");
  let stored = storedSubscriptions();
  for (let i = 0; i < 20 && stored < 1; i++) {
    await page.waitForTimeout(500);
    stored = storedSubscriptions();
  }
  note("the subscription reached the api", stored >= 1, `push_subscriptions rows: ${stored}`);
  await page.locator("#push-prompt").waitFor({ state: "hidden", timeout: 10000 }).catch(() => {});
  note("the offer is gone once permission is granted", await page.locator("#push-prompt").isHidden());
  await page.screenshot({ path: `${OUT}/2-after-grant.png` });

  // ---- 2. the user's own comment is quiet ----
  await clearNotifications();
  seedStep("own", JSON.stringify(seeded));
  const quiet = await waitForNotification(`build-task-${seeded.taskId}`, 8000);
  note("the user's own comment on a watched task pushes nothing", quiet === null, quiet ? JSON.stringify(quiet) : "8 s, nothing shown");

  // ---- 3. an agent answering, and commenting on the watched task ----
  await clearNotifications();
  tellTheHarness(seeded.taskId);
  seedStep("agent", JSON.stringify(seeded));
  const agent = await waitForNotification(`build-task-${seeded.entityId}`, 60000);
  const task = await waitForNotification(`build-task-${seeded.taskId}`, 15000);
  const conversation = agent && task ? "" : `; the conversation holds ${JSON.stringify(seedStep("thread", JSON.stringify(seeded)))}`;
  note("the agent's report arrives as a notification", Boolean(agent), agent ? `${agent.ms} ms: "${agent.title}: ${agent.body}" → ${agent.url}` : `none in 60 s${conversation}`);
  note("the agent notification's copy is generic", agent?.body === "An agent needs you");
  note("the agent's comment on the watched task arrives as a notification", Boolean(task), task ? `"${task.title}: ${task.body}" → ${task.url}` : `none${conversation}`);
  note("the task notification names a task", task?.body === "New activity on a task");
  await page.screenshot({ path: `${OUT}/3-notified.png` });

  // ---- 4. each deep link opens its surface ----
  const opens = async (url, expected) => {
    await page.goto(`${APP}${url}`, { waitUntil: "load" });
    return page.waitForFunction((part) => location.hash.includes(part) && location.hash, expected, { timeout: 30000 })
      .then((handle) => handle.jsonValue()).catch(() => null);
  };
  if (task?.url) {
    const landed = await opens(task.url, `/tasks/${seeded.taskId}`);
    note("the task deep link opens the task on the tracker", Boolean(landed), landed || page.url());
    await page.waitForTimeout(1500);
    await page.screenshot({ path: `${OUT}/4-task-deep-link.png` });
  }
  if (agent?.url) {
    const landed = await opens(agent.url, `/workspace/${seeded.workspaceId}`);
    note("the agent deep link opens its workspace", Boolean(landed), landed || page.url());
    await page.waitForTimeout(1500);
    await page.screenshot({ path: `${OUT}/5-agent-deep-link.png` });
  }
} finally {
  writeFileSync(`${OUT}/results.json`, JSON.stringify({ seeded, results }, null, 2));
  await context.close();
}
const failed = results.filter((result) => !result.ok);
console.log(failed.length ? `${failed.length} FAILED` : `all ${results.length} passed`);
process.exit(failed.length ? 1 : 0);
