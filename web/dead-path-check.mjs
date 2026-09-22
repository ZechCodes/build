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
// # Why the browser is made to keep believing ICE
//
// `docker pause` freezes the bridge so completely that it stops answering ICE's
// own consent checks, and the browser notices within a few seconds: the
// connection goes `disconnected`, core/peerLink.js starts an ICE restart, and
// that machinery — which predates this issue — is what handles it. Zech's fault
// was the opposite and is the whole reason #30 exists: his consent checks WERE
// being answered, ICE said `connected` for the full 105 seconds, and nothing
// above it could tell that SCTP was delivering none of his frames.
//
// So a paused container on its own reproduces the symptom and not the fault, and
// a run over it would pass or fail for reasons that have nothing to do with the
// probe. `RTCPeerConnection` is therefore patched in an init script to keep
// reporting the state it reached once it has been connected — the same lever
// web/relayed-path.mjs uses to reproduce his conditions without changing a line
// of the product. With it, the browser's own ICE is blind exactly as his was, and
// the probe is the only thing left that can notice. `KEEP_ICE=0` turns it off, to
// watch the pause without it.
//
// # Why the pause and not `docker stop`
//
// Same reason as dropped-read-check: `stop` deregisters the device, the account
// stops listing it online, and the supervisor stands down — which is a machine
// that is GONE, not a machine whose path died under it. A paused container holds
// its registration and answers nothing, which is Zech's tablet exactly.

import { existsSync, readFileSync } from "node:fs";
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
  // One literal pattern per constant rather than one built from its name: a
  // regex assembled from a variable is a finding even when the variable is a
  // constant three lines up, and three literals are no harder to read.
  const read = (source, name, pattern) => {
    const found = source.match(pattern);
    if (!found) throw new Error(`no ${name} in the SPA — this check needs it`);
    return Number(found[1]);
  };
  return {
    pathDeadline: read(rpc, "DEFAULT_RPC_TIMEOUT_MS", /DEFAULT_RPC_TIMEOUT_MS\s*=\s*(\d+)/),
    ping: read(liveness, "PING_TIMEOUT_MS", /PING_TIMEOUT_MS\s*=\s*(\d+)/),
    frameProof: read(liveness, "FRAME_PROOF_OF_LIFE_MS", /FRAME_PROOF_OF_LIFE_MS\s*=\s*(\d+)/),
  };
})();
/** What the old behaviour cost: SCTP gave up on its retransmits after about
 *  this long, and that was the whole of the client's recovery. */
const SCTP_GAVE_UP_AFTER_MS = 105000;

const seed = JSON.parse(readFileSync("/tmp/live-seed.json", "utf8"));
const workspace = seed.workspaces[0];
const docker = (command) =>
  execSync(`echo '${command}' | newgrp docker 2>&1`, { encoding: "utf8", shell: "/bin/bash" });

/** Pause and unpause, with the unpause guaranteed.
 *
 *  A run that is killed between the two leaves the whole compose stack frozen
 *  for whoever holds it next, and the next person's failure looks like their own.
 *  So the unpause is idempotent, runs on the way out however this process ends,
 *  and is the only thing that ever calls `docker unpause` here. */
let bridgeIsPaused = false;
const pauseBridge = () => {
  docker(`docker pause ${BRIDGE}`);
  bridgeIsPaused = true;
};
const unpauseBridge = () => {
  if (!bridgeIsPaused) return;
  bridgeIsPaused = false;
  try {
    docker(`docker unpause ${BRIDGE}`);
  } catch (error) {
    console.error(`could not unpause ${BRIDGE}: ${error?.message || error}`);
  }
};
for (const signal of ["exit", "SIGINT", "SIGTERM"]) process.on(signal, unpauseBridge);

/** The picture this run sends, as bytes rather than as a fixture file: a 1×1
 *  PNG. The size is beside the point — what is being tested is which of three
 *  states the figure ends up in, not how many bytes cross. */
const PNG_1X1_B64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==";

const results = [];
const record = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
};

/** Keep the browser saying what ICE last said, once it has said `connected`.
 *
 *  Patched on the prototype so it applies to whatever the SPA constructs, and
 *  only ever LATCHES a good state — a connection on its way up still reports the
 *  truth, so negotiation is untouched and only the noticing is blinded. */
const PIN_ICE = `(() => {
  const proto = RTCPeerConnection.prototype;
  const wasUp = new WeakSet();
  for (const field of ["iceConnectionState", "connectionState"]) {
    const real = Object.getOwnPropertyDescriptor(proto, field);
    if (!real || !real.get) continue;
    Object.defineProperty(proto, field, {
      configurable: true,
      get() {
        const said = real.get.call(this);
        if (said === "connected" || said === "completed") {
          wasUp.add(this);
          return said;
        }
        return wasUp.has(this) ? "connected" : said;
      },
    });
  }
})();`;

const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
// One explicit context: browser.newPage() wraps a context with no session
// cookie, and a page without it lands on the sign-in gate.
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
if (process.env.KEEP_ICE !== "0") await context.addInitScript(PIN_ICE);
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
// The report is `{ since, dropped, events }` since #60; a bare array before.
// Neither shape means the page changed under this script, and walking nothing
// would read as a clean run, so it stops.
const diagnostics = async () => {
  const report = await page.evaluate(() => globalThis.buildConnectionDiagnostics?.() ?? null);
  const history = Array.isArray(report) ? report : report?.events;
  if (!Array.isArray(history)) {
    throw new Error(`buildConnectionDiagnostics() answered ${JSON.stringify(report)}, which is neither the event list nor a report with events, so this check cannot read what the page recorded.`);
  }
  return history;
};
const probes = async () => (await diagnostics()).filter((entry) => entry.event === "path-probe");
const sessionsConnected = async () =>
  (await diagnostics()).filter((entry) => entry.event === "connected").length;
const recoveryText = () =>
  page.evaluate(() => document.querySelector(".chat-recovery")?.innerText?.replace(/\s+/g, " ").trim() || "");
// The rail's composer, by the ids it is mounted under (core/agentRail.js's
// COMPOSER_IDS and core/composer.js's composerPartIds). Not "the first textarea
// on the page": a workspace also carries the changes-comment box, and Enter does
// not send in either of them — the arrow does.
const RAIL = { input: "#railinput", send: "#railsend", file: "#railinputfile", tray: "#railinputtray" };

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
await wait(12000);
const liveSessions = await sessionsConnected();
record("the workspace stands up with the bridge answering", liveSessions > 0, `${liveSessions} connected session(s) recorded`);
const composerThere = await page.locator(RAIL.input).count();
record("the conversation's composer is on screen", composerThere === 1, `${composerThere} rail composer(s)`);

// ── 2. a message with a picture, written and uploaded while the path is fine ─
//
// This is the order Zech's report happened in and it matters: the bytes go up
// BEFORE the message does, so the attachment was safely on the bridge and it was
// the FETCH of it back — `thread.attachment`, for the preview of his own just-sent
// message — that died. Uploading before the pause is therefore not a shortcut; it
// is the case.
const stamp = Date.now().toString(36);
await page.setInputFiles(RAIL.file, {
  name: `dead-path-${stamp}.png`,
  mimeType: "image/png",
  buffer: Buffer.from(PNG_1X1_B64, "base64"),
});
const uploaded = await until(
  async () => page.evaluate((tray) => !document.querySelector(tray)?.hidden, RAIL.tray),
  { every: 500, forMs: 30000 },
);
record("the picture uploads while the path is still carrying", Boolean(uploaded.found), uploaded.found ? "in the tray" : "never reached the tray");
await page.locator(RAIL.input).fill(`dead-path-check ${stamp}`);

// ── 3. the path dies, and then the send goes out over it ────────────────────
console.log(`\npausing ${BRIDGE} — registered, answering nothing\n`);
pauseBridge();
const beforeProbes = (await probes()).length;
await page.locator(RAIL.send).click();

const judged = await until(async () => {
  const written = (await probes()).slice(beforeProbes);
  return written.some((entry) => entry.state === "dead") ? written : null;
}, { every: 1000, forMs: 90000 });

const asked = judged.found?.find((entry) => entry.state === "asked");
record(
  "the probe asks the wire once the send burns its path deadline",
  Boolean(asked),
  asked ? `asked about ${asked.method}, vouched=${asked.vouched}` : `states seen: ${JSON.stringify((await probes()).map((e) => e.state))}`,
);
record(
  "…and judges the path dead rather than waiting for SCTP",
  judged.found !== null && judged.afterMs < SCTP_GAVE_UP_AFTER_MS,
  judged.found === null
    ? "no verdict in 90s"
    : `dead at +${Math.round(judged.afterMs / 1000)}s, where SCTP took ~${SCTP_GAVE_UP_AFTER_MS / 1000}s`,
);
// The verdict cannot honestly come before the deadline that triggered it and the
// ping that answered it have both elapsed.
const floorMs = timing.pathDeadline + timing.ping;
record(
  `the verdict waits out the deadline and the ping first (${floorMs} ms)`,
  judged.found !== null && judged.afterMs >= floorMs,
  `judged at ${judged.afterMs} ms, floor ${floorMs} ms (deadline ${timing.pathDeadline} + ping ${timing.ping}, frame window ${timing.frameProof})`,
);
record(
  "the restart is recorded beside the verdict",
  (await probes()).some((entry) => entry.state === "restarting"),
  JSON.stringify((await probes()).map((entry) => entry.state)),
);

const stranded = await until(async () => {
  const shown = await recoveryText();
  return /uncertain/i.test(shown) ? shown : null;
}, { every: 1000, forMs: 30000 });
record(
  "the send that died is marked uncertain, with Check delivery on it",
  stranded.found !== null && /check delivery/i.test(stranded.found),
  JSON.stringify(stranded.found || (await recoveryText())).slice(0, 200),
);

const figures = () =>
  page.evaluate(() => [...document.querySelectorAll(".thread-attachment-figure")].map((one) => one.className));
const waiting = await until(async () => {
  const shown = await figures();
  return shown.some((className) => className.includes("waiting")) ? shown : null;
}, { every: 1000, forMs: 40000 });
record(
  "the picture the dead path ate says loading, not unavailable",
  waiting.found !== null,
  JSON.stringify(waiting.found || (await figures())),
);
record(
  "…and nothing is marked unavailable",
  !(await figures()).some((className) => className.includes("unavailable")),
  JSON.stringify(await figures()),
);

// ── 4. the machine comes back ───────────────────────────────────────────────
console.log(`\nunpausing ${BRIDGE}\n`);
const sessionsBefore = await sessionsConnected();
unpauseBridge();
const resumedAt = Date.now();

const reconnected = await until(async () => (await sessionsConnected()) > sessionsBefore, { every: 1000, forMs: 90000 });
record(
  "the supervisor lands a new session",
  Boolean(reconnected.found),
  reconnected.found ? `connected again at +${Math.round(reconnected.afterMs / 1000)}s` : "never reconnected in 90s",
);

// The whole point of point 2: this clears with nobody pressing anything.
const settled = await until(async () => ((await recoveryText()) === "" ? "clear" : null), { every: 1000, forMs: 90000 });
record(
  "the uncertain post settles itself, with no press",
  settled.found === "clear",
  settled.found === "clear"
    ? `cleared at +${Math.round(settled.afterMs / 1000)}s after the unpause`
    : `still showing ${JSON.stringify(await recoveryText()).slice(0, 200)}`,
);
const landed = await until(
  async () => page.evaluate((tag) => document.body.innerText.includes(tag), `dead-path-check ${stamp}`),
  { every: 1000, forMs: 60000 },
);
record(
  "…and the message is in the conversation",
  Boolean(landed.found),
  landed.found ? `on screen at +${Math.round((Date.now() - resumedAt) / 1000)}s` : "never appeared",
);

const released = (await diagnostics()).filter((entry) => entry.event === "attachments-released");
record(
  "the reconnect releases the pictures the dead path ate",
  released.length > 0,
  JSON.stringify(released.map((entry) => ({ conversations: entry.conversations, paths: entry.paths }))),
);
const refetched = await until(
  async () => page.evaluate(() => [...document.querySelectorAll("img.thread-attachment-image")].some((img) => img.getAttribute("src"))),
  { every: 1000, forMs: 90000 },
);
record(
  "…and the picture arrives",
  Boolean(refetched.found),
  refetched.found ? `filled at +${Math.round(refetched.afterMs / 1000)}s after the release` : `figures: ${JSON.stringify(await figures())}`,
);

// What the page recorded about the two decisions, printed whether or not the run
// passed: a FAIL whose reason is in the history and not on the screen is a run
// somebody has to do again by hand.
const history = await diagnostics();
const interesting = history.filter((entry) =>
  ["path-probe", "attachment-failed", "attachments-released"].includes(entry.event));
console.log("\n──────── what the page recorded ────────");
console.log(`(the history holds the last ${history.length} events; a dead window with reconnects in it can overflow it)`);
for (const entry of interesting) {
  const detail = Object.entries(entry)
    .filter(([field]) => !["at", "connection", "event"].includes(field))
    .map(([field, value]) => `${field}=${JSON.stringify(value)}`)
    .join(" ");
  console.log(`  ${entry.event.padEnd(21)} ${detail}`);
}

await browser.close();
console.log("\n──────── summary ────────");
for (const { name, ok, detail } of results) console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
const passed = results.filter((one) => one.ok).length;
console.log(`\n${passed}/${results.length} passed`);
process.exit(passed === results.length ? 0 : 1);
