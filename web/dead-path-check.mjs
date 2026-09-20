// #30 on a real stack: a session whose path has silently died is not kept for
// two minutes, and what the dead window stranded settles itself afterwards.
//
//   ISSUES_REPO=<this checkout> node web/dead-path-check.mjs
//
// Reads /tmp/live-seed.json, so run web/live-seed.mjs first and write its SEED
// line there. Exits non-zero on a failed check.
//
// # Why this exists beside web/dropped-read-check.mjs and not inside it
//
// That check is about a READ that failed: the cached copy is kept, the surface
// is marked, and the read runs again on reconnect. This one is about the SESSION
// under it, which is the opposite end of the same fault — and its three
// assertions need a state that check deliberately does not create: a post caught
// mid-send and an attachment caught mid-fetch. Bolting them on would have made
// one script that fails for four unrelated reasons.
//
// # What only a real stack can show
//
// The unit tests drive the probe over injected timers and they cannot know the
// two things that matter here. First, WHEN: `docker pause` freezes the bridge
// with its registration intact and its ICE consent unanswered, and the order in
// which the RPC deadline, the probe, the browser's own ICE state change and the
// supervisor's first attempt actually land is not something a fake clock has an
// opinion about. Second, WHETHER THE RECONNECT IS REAL: a severed session is
// only worth anything if a new one lands on the other side of it, and that
// involves a relay, a rendezvous, two handshakes and a greeting.
//
// # Why the pause and not `docker stop`
//
// Same reason as dropped-read-check: `stop` deregisters the device, the account
// stops listing it online, and the supervisor stands down — which is a machine
// that is GONE, not a machine whose path died under it. A paused container holds
// its registration and answers nothing, which is Zech's tablet exactly.

import { randomUUID } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { execSync } from "node:child_process";
import { chromium } from "/home/zech/.local/share/mise/installs/npm-playwright/1.63.0/node_modules/playwright/index.mjs";

const APP = process.env.APP_URL || "http://localhost:8090";
const BRIDGE = process.env.BRIDGE_CONTAINER || "deploy-bridge-1";

/** The ONE checkout this runs off — refused rather than defaulted, for the same
 *  reason the other checks refuse it: there is more than one build-web checkout
 *  on this machine and a forgotten variable would pass against a bridge that was
 *  never under test. */
const REPO = process.env.ISSUES_REPO;
if (!REPO) {
  console.error("set ISSUES_REPO to the checkout under test — this run must not mix checkouts");
  process.exit(2);
}
if (!existsSync(`${REPO}/deploy/compose.real.yml`)) {
  console.error(`no compose file under ${REPO} — is ISSUES_REPO a build-web checkout?`);
  process.exit(2);
}

/** The three numbers this run waits out, read from the modules that arm them so
 *  the script cannot drift from the product. */
const timing = (() => {
  const rpc = readFileSync(`${REPO}/spa/src/core/sessionRpc.js`, "utf8");
  const liveness = readFileSync(`${REPO}/spa/src/core/pathLiveness.js`, "utf8");
  const read = (source, name) => {
    const found = source.match(new RegExp(`${name}\\s*=\\s*(\\d+)`));
    if (!found) throw new Error(`no ${name} in the SPA — this check needs it`);
    return Number(found[1]);
  };
  return {
    pathDeadline: read(rpc, "DEFAULT_RPC_TIMEOUT_MS"),
    ping: read(liveness, "PING_TIMEOUT_MS"),
    frameProof: read(liveness, "FRAME_PROOF_OF_LIFE_MS"),
  };
})();
/** What the old behaviour cost: SCTP gave up on its retransmits after about
 *  this long, and that was the whole of the client's recovery. */
const SCTP_GAVE_UP_AFTER_MS = 105000;

const seed = JSON.parse(readFileSync("/tmp/live-seed.json", "utf8"));
const workspace = seed.workspaces[0];
const docker = (command) =>
  execSync(`echo '${command}' | newgrp docker 2>&1`, { encoding: "utf8", shell: "/bin/bash" });

/** One bridge RPC from inside the qa container, which is where the compose
 *  network and the harness deps are. `--no-deps` so this cannot recreate the
 *  app or bridge another checkout's compose file started. */
function call(method, params) {
  const script = `import * as transport from "@build/secure-transport"; import { openDeviceLink, openRendezvous } from "./client.mjs"; import { loginWithDummy } from "./skrift-auth.mjs";
const { cookie, mintGatewayToken } = await loginWithDummy(process.env.API_URL, { email: "qa@localhost" });
const rendezvous = await openRendezvous({ relayUrl: process.env.RELAY_URL, mintGatewayToken });
const link = await openDeviceLink({ rendezvous, transport, apiUrl: process.env.API_URL, cookie });
const r = await link.session.call(${JSON.stringify(method)}, ${JSON.stringify(params)});
console.log("RESULT " + JSON.stringify(r)); process.exit(0);`;
  writeFileSync("/tmp/dead-path-call.mjs", script);
  const out = execSync(
    `echo 'docker compose -f ${REPO}/deploy/compose.real.yml --profile qa run --rm -T --no-deps qa node --input-type=module - < /tmp/dead-path-call.mjs' | newgrp docker 2>&1`,
    { encoding: "utf8", shell: "/bin/bash" },
  );
  const line = out.split("\n").find((one) => one.startsWith("RESULT "));
  if (!line) throw new Error(`no result from ${method}: ${out.trim().slice(-300)}`);
  return JSON.parse(line.slice("RESULT ".length));
}

/** A picture in the conversation, put there before the browser looks — the
 *  figure whose fetch the dead window is going to eat. Small, because the point
 *  is which of three states the figure ends up in and not how many bytes cross.
 *  A 1×1 PNG, as bytes rather than as a file, so the script carries no fixture. */
const PNG_1X1_B64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==";

function seedPicture() {
  const uploaded = call("thread.attach", {
    entity_id: workspace.entityId,
    filename: `dead-path-${Date.now().toString(36)}.png`,
    content_b64: PNG_1X1_B64,
  });
  call("thread.post", {
    entity_id: workspace.entityId,
    agent_id: workspace.agentId,
    conversation_id: workspace.conversationId,
    operation_id: randomUUID(),
    body: "A picture for web/dead-path-check.mjs.",
    attachments: [{ path: uploaded.path, name: uploaded.name }],
  });
  return uploaded;
}

const results = [];
const record = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
};

const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
// One explicit context: browser.newPage() wraps a context with no session
// cookie, and a page without it lands on the sign-in gate.
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
await page.fill('input[name="email"]', "qa@localhost");
await Promise.all([
  page.waitForNavigation({ waitUntil: "load" }),
  page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
]);

/** The diagnostic history as the page holds it — the same dump
 *  `buildConnectionDiagnostics()` gives support, which is what Settings →
 *  Diagnostics renders. */
const diagnostics = () => page.evaluate(() => globalThis.buildConnectionDiagnostics?.() || []);
const probes = async () => (await diagnostics()).filter((entry) => entry.event === "path-probe");
const sessionsConnected = async () =>
  (await diagnostics()).filter((entry) => entry.event === "connected").length;
const recoveryText = () =>
  page.evaluate(() => document.querySelector(".chat-recovery")?.innerText?.replace(/\s+/g, " ").trim() || "");
const composer = ".rail-composer textarea, .composer textarea, textarea";

const wait = (ms) => page.waitForTimeout(ms);
const until = async (answer, { every = 1000, forMs = 60000 } = {}) => {
  const started = Date.now();
  for (;;) {
    const found = await answer();
    if (found) return { found, afterMs: Date.now() - started };
    if (Date.now() - started > forMs) return { found: null, afterMs: Date.now() - started };
    await wait(every);
  }
};

// ── 1. stand on the workspace's conversation, live ──────────────────────────
const route = `${APP}/app/#/project/${seed.projectId}/workspace/${workspace.workspaceId}`;
await page.goto(route, { waitUntil: "load" });
await wait(10000);
const liveSessions = await sessionsConnected();
record("the workspace stands up with the bridge answering", liveSessions > 0, `${liveSessions} connected session(s) recorded`);
const composerThere = await page.locator(composer).count();
record("the conversation's composer is on screen", composerThere > 0, `${composerThere} composer(s)`);

// ── 2. the path dies under a send ───────────────────────────────────────────
console.log(`\npausing ${BRIDGE} — registered, answering nothing\n`);
docker(`docker pause ${BRIDGE}`);
const pausedAt = Date.now();
const beforeProbes = (await probes()).length;

// A send is what puts a frame on the wire and starts the deadline that asks the
// question. The body is tagged so the resolution can be recognised afterwards.
const stamp = Date.now().toString(36);
await page.locator(composer).first().fill(`dead-path-check ${stamp}`);
await page.keyboard.press("Enter");

const judged = await until(async () => {
  const written = (await probes()).slice(beforeProbes);
  return written.some((entry) => entry.state === "dead") ? written : null;
}, { every: 2000, forMs: 90000 });

const askedAt = judged.found?.find((entry) => entry.state === "asked");
record(
  "the probe asks the wire once the send burns its path deadline",
  Boolean(askedAt),
  askedAt ? `asked about ${askedAt.method} at +${Math.round(judged.afterMs / 1000)}s, vouched=${askedAt.vouched}` : "never asked",
);
record(
  "…and judges the path dead rather than waiting for SCTP",
  judged.found !== null && judged.afterMs < SCTP_GAVE_UP_AFTER_MS,
  judged.found === null
    ? "no verdict in 90s"
    : `dead at +${Math.round(judged.afterMs / 1000)}s, where SCTP took ~${SCTP_GAVE_UP_AFTER_MS / 1000}s`,
);
// The verdict cannot honestly come before the deadline that triggered it, the
// frame proof window, and the ping have all elapsed.
const floorMs = timing.pathDeadline + timing.ping;
record(
  `the verdict waits out the deadline and the ping first (${floorMs} ms)`,
  judged.afterMs >= floorMs,
  `judged at ${judged.afterMs} ms, floor ${floorMs} ms (deadline ${timing.pathDeadline} + ping ${timing.ping}, frame window ${timing.frameProof})`,
);
record(
  "the restart is recorded beside the verdict",
  (await probes()).some((entry) => entry.state === "restarting"),
  JSON.stringify((await probes()).map((entry) => entry.state)),
);
const stranded = await recoveryText();
record(
  "the send that died is marked uncertain, with Check delivery on it",
  /uncertain/i.test(stranded) && /check delivery/i.test(stranded),
  JSON.stringify(stranded).slice(0, 180),
);

// ── 3. the machine comes back ───────────────────────────────────────────────
console.log(`\nunpausing ${BRIDGE}\n`);
const sessionsBefore = await sessionsConnected();
docker(`docker unpause ${BRIDGE}`);
const resumedAt = Date.now();

const reconnected = await until(async () => (await sessionsConnected()) > sessionsBefore, { every: 2000, forMs: 90000 });
record(
  "the supervisor lands a new session",
  Boolean(reconnected.found),
  reconnected.found ? `connected again at +${Math.round(reconnected.afterMs / 1000)}s` : "never reconnected in 90s",
);

// The whole point of point 2: this clears with nobody pressing anything.
const settled = await until(async () => {
  const shown = await recoveryText();
  return shown === "" ? "clear" : null;
}, { every: 2000, forMs: 90000 });
record(
  "the uncertain post settles itself, with no press",
  settled.found === "clear",
  settled.found === "clear"
    ? `cleared at +${Math.round(settled.afterMs / 1000)}s after the unpause`
    : `still showing ${JSON.stringify(await recoveryText()).slice(0, 180)}`,
);
// It settled one way or the other: either the bridge had it, or it was re-sent.
// Both end with the message in the transcript, which is the reader's test.
const landed = await until(
  async () => page.evaluate((tag) => document.body.innerText.includes(tag), `dead-path-check ${stamp}`),
  { every: 2000, forMs: 60000 },
);
record(
  "…and the message is in the conversation",
  Boolean(landed.found),
  landed.found ? `on screen at +${Math.round((Date.now() - resumedAt) / 1000)}s` : "never appeared",
);

// ── 4. a picture whose fetch the dead window eats ───────────────────────────
//
// A reload is how this is reproduced honestly. The bytes are cached per
// conversation for the life of the tab, so a picture already on screen would not
// be fetched again — and Zech's case is exactly the fetch that goes out for the
// first time while the path is dead. A reload keeps the cache-first paint (same
// origin, same IndexedDB) and takes a fresh attachment cache with it, which is
// the state that used to leave the figure saying "unavailable" until a hard
// refresh.
console.log("\nseeding a picture, then reloading with the bridge paused\n");
seedPicture();
await page.goto(route, { waitUntil: "load" });
await until(async () => page.locator("img.thread-attachment-image").count().then((n) => n > 0), { every: 1000, forMs: 30000 });
const shownLive = await until(
  async () => page.evaluate(() => [...document.querySelectorAll("img.thread-attachment-image")].some((img) => img.getAttribute("src"))),
  { every: 1000, forMs: 30000 },
);
record("the seeded picture loads with the bridge answering", Boolean(shownLive.found), shownLive.found ? "filled" : "never filled");

docker(`docker pause ${BRIDGE}`);
await page.reload({ waitUntil: "load" });
const figures = () =>
  page.evaluate(() => [...document.querySelectorAll(".thread-attachment-figure")].map((one) => one.className));
const waiting = await until(async () => {
  const shown = await figures();
  return shown.some((className) => className.includes("waiting")) ? shown : null;
}, { every: 1000, forMs: 60000 });
record(
  "a picture the dead path ate says loading, not unavailable",
  waiting.found !== null,
  waiting.found ? JSON.stringify(waiting.found) : JSON.stringify(await figures()),
);
record(
  "…and is not marked unavailable anywhere",
  !(await figures()).some((className) => className.includes("unavailable")),
  JSON.stringify(await figures()),
);

docker(`docker unpause ${BRIDGE}`);
const refetched = await until(
  async () => page.evaluate(() => [...document.querySelectorAll("img.thread-attachment-image")].some((img) => img.getAttribute("src"))),
  { every: 2000, forMs: 90000 },
);
record(
  "the reconnect asks for it again and the picture arrives",
  Boolean(refetched.found),
  refetched.found ? `filled at +${Math.round(refetched.afterMs / 1000)}s after the unpause` : "never filled in 90s",
);

void pausedAt;
await browser.close();
console.log("\n──────── summary ────────");
for (const { name, ok, detail } of results) console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
const passed = results.filter((one) => one.ok).length;
console.log(`\n${passed}/${results.length} passed`);
process.exit(passed === results.length ? 0 : 1);
