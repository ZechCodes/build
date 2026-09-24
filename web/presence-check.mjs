// Does the browser stop believing in a machine that has gone?
//
// Against the compose stack: log in, land in the app on a live bridge, then
// stop the bridge container and watch two things the fix is about —
//   1. the account's own list (`GET /api/devices`, read from the page with the
//      reader's session) flips that machine to offline inside the api's 90 s
//      window, and the client's own view of it goes with it;
//   2. the client stops asking the relay for it, rather than dialling a machine
//      that is not there for as long as the tab is open.
//
// The stack has to be up, the bridge paired and `live-seed.mjs` run first, so
// there is a workspace to open and a machine that is genuinely answering:
//
//   docker compose -f deploy/compose.real.yml up -d --build
//   docker compose -f deploy/compose.real.yml --profile qa run --rm qa node pair.mjs
//   docker compose -f deploy/compose.real.yml --profile qa run --rm qa node live-seed.mjs
//   node presence-check.mjs            # from Build/web, on the host
//
// It stops the bridge container and leaves it stopped; `docker compose start
// bridge` puts it back.
import { execSync } from "node:child_process";
import { chromium } from "playwright";

const APP = process.env.APP || "http://localhost:8090";
const EMAIL = process.env.QA_EMAIL || "qa@localhost";
const WINDOW_S = 90; // buildapp.presence.ONLINE_WINDOW

const docker = (cmd) => execSync(`echo '${cmd}' | newgrp docker`, { encoding: "utf8" });
const now = () => new Date().toISOString().slice(11, 19);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const note = (ok, name, detail = "") => {
  results.push({ ok, name, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? "  — " + detail : ""}`);
};

/** Every line the relay has logged so far. The dials a browser makes are in
 *  here as `client N: connected` and `rejected session to device`. */
const relayLines = () => docker("docker logs deploy-relay-1 2>&1").split("\n").filter(Boolean);
const dialsSoFar = () => relayLines().filter((line) => /client \d+: connected|rejected session to device/.test(line)).length;

const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
page.on("pageerror", (e) => console.log("  [pageerror]", e.message));

// ---- log in and land in the app ------------------------------------------
await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
await page.fill('input[name="email"]', EMAIL);
await page.fill('input[name="name"]', "Presence Check");
await Promise.all([
  page.waitForNavigation({ waitUntil: "load" }),
  page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
]);
await page.goto(`${APP}/app/`, { waitUntil: "load" });

/** The account's list as the reader's own session sees it. */
const accountDevices = async () => {
  for (let tries = 0; tries < 5; tries += 1) {
    try {
      return await page.evaluate(async () => (await (await fetch("/api/devices")).json()).devices);
    } catch { await sleep(2000); }
  }
  throw new Error("the api never answered the page");
};

const landed = await page.waitForFunction(
  () => !document.body.classList.contains("gated") && document.querySelectorAll("#devpick").length > 0,
  null,
  { timeout: 60000 },
).then(() => true).catch(() => false);
note(landed, "the app is standing on a machine", landed ? "" : "never left the gate");

// The bridge beats on its first relay authentication, and the client re-reads
// presence every 15 s, so give both a chance before asking anything of either.
let devices = [];
let online = false;
for (let waited = 0; waited < 120 && !online; waited += 5) {
  devices = await accountDevices();
  online = devices.some((d) => d.status === "online");
  if (!online) await sleep(5000);
}
note(online, "the account lists the bridge online", JSON.stringify(devices.map((d) => [d.name, d.status])));

// A session, not just a listing: the client has to have actually reached the
// machine, or "it stops dialling" below is a claim about nothing.
const clientConnections = () => relayLines().filter((line) => /client \d+: connected/.test(line)).length;
let dialled = 0;
for (let waited = 0; waited < 90 && dialled === 0; waited += 3) {
  dialled = clientConnections();
  if (dialled === 0) await sleep(3000);
}
const painted = await page.waitForFunction(
  () => document.body.textContent.includes("live-alpha") || document.body.textContent.includes("live-beta"),
  null,
  { timeout: 60000 },
).then(() => true).catch(() => false);
note(dialled > 0 && painted, "the client reached that machine over the relay",
  `${dialled} client connections logged; rail ${painted ? "painted the seeded workspaces" : "painted nothing"}`);

// Open the workspace the qa bridge keeps, so a real surface is mounted and the
// cache sync layer is running against that machine.
const row = page.locator("#inbox-list .inbox-entry", { hasText: "live-alpha" }).first();
await row.click({ timeout: 20000 }).catch((error) => console.log("  [row click]", error.message));
await sleep(4000);
const opened = await page.evaluate(() => location.hash.includes("/workspace/"));
note(opened, "a workspace is open on that machine", await page.evaluate(() => location.hash));

// ---- stop the machine -----------------------------------------------------
const dialsBeforeStop = dialsSoFar();
console.log(`${now()}  stopping deploy-bridge-1 (relay dials so far: ${dialsBeforeStop})`);
docker("docker stop deploy-bridge-1");
const stoppedAt = Date.now();

// ---- 1. does the account let it go, and does the client follow? -----------
let flippedAfter = null;
let clientSaysOffline = null;
while ((Date.now() - stoppedAt) / 1000 < WINDOW_S + 45) {
  devices = await accountDevices();
  if (flippedAfter === null && devices.every((d) => d.status !== "online")) {
    flippedAfter = Math.round((Date.now() - stoppedAt) / 1000);
    console.log(`${now()}  /api/devices says offline, ${flippedAfter}s after the stop`);
  }
  if (flippedAfter !== null) {
    // The client's own picture: the picker wears the offline mark, and nothing
    // can answer any more.
    clientSaysOffline = await page.evaluate(() =>
      Boolean(document.querySelector('#devpick [aria-label="Device offline"]')) ||
      (window.App?.devices || []).every((d) => d.status !== "online"));
    if (clientSaysOffline) break;
  }
  await sleep(5000);
}
note(flippedAfter !== null && flippedAfter <= WINDOW_S + 30,
  `the account's list flips to offline inside the window`,
  flippedAfter === null ? "never flipped" : `${flippedAfter}s (window ${WINDOW_S}s)`);
note(Boolean(clientSaysOffline), "the client's own view follows it offline");

// ---- 2. does it stop dialling? -------------------------------------------
await sleep(30000); // let any in-flight recovery attempt land and stand down
const settled = dialsSoFar();
console.log(`${now()}  relay dials at settle point: ${settled}`);
await sleep(75000); // three presence polls and two full recovery backoff ceilings
const after = dialsSoFar();
note(after === settled, "the client stops asking the relay for a machine that has gone",
  `${after - settled} new dials in the 75 s after it settled (${settled} total by then)`);

const tail = relayLines().slice(-6);
console.log("  relay tail:\n    " + tail.join("\n    "));

await browser.close();
console.log("\n" + results.map((r) => `${r.ok ? "PASS" : "FAIL"} ${r.name}`).join("\n"));
process.exit(results.every((r) => r.ok) ? 0 : 1);
