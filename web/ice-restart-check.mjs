// #131 §5: an ICE restart on a relayed path lands within 15 s.
//
// The phone's sessions ride TURN, and when a path fails the app restarts ICE:
// a fresh gather, a fresh offer marked `iceRestart`, sent to the bridge over
// the session's own signaling, answered, and checked to carry. The Node soak
// (liveness-soak.mjs) cannot do this — node-datachannel's `restartIce()` throws
// "Not implemented" — so this runs the real app in Chromium against the same
// stack, TURN-only, and puts it through one restart the way the app would go
// through one for real:
//
//   1. every candidate forced through the stack's coturn (the two levers of
//      relayed-path.mjs: the ICE-servers route answered with coturn, and
//      `iceTransportPolicy: "relay"`), so the path is the phone's;
//   2. once connected and settled, the peer reports `failed` ONE time — the
//      only thing synthetic here — and the app's own recovery takes it from
//      there: `renegotiate("restart")` in spa/src/core/peerLink.js, a real
//      `iceRestart` offer, a real answer from the bridge, a real set of checks,
//      and the app's own "does it carry" probe before it says `connected`;
//   3. the app's diagnostics (`buildConnectionDiagnostics()`, what Settings →
//      Diagnostics shows) time it: `restarting` to the restart's `connected`.
//
// Exit 0 when the restart landed within RESTART_DEADLINE_MS on a relayed path
// and the app held it for HOLD_MS after; 1 when it did not; 2 when the run
// could not get as far as a restart (no connection to begin with).
//
// It runs inside mcr.microsoft.com/playwright with the host's network, so it
// reaches the stack's published ports and coturn's address on the compose
// network (scripts/liveness-gate.sh starts it that way):
//
//   APP=http://localhost:8128  TURN_HOST=<coturn IP>  TURN_USER / TURN_PASSWORD
//   SETTLE_MS=20000   how long the session carries before the restart
//   RESTART_DEADLINE_MS=15000   HOLD_MS=20000   CHROMIUM_PATH (optional)

import { chromium } from "playwright";

const num = (name, fallback) => Number(process.env[name] || fallback);
const APP = process.env.APP || "http://localhost:8128";
const EMAIL = process.env.QA_EMAIL || "qa@localhost";
const SETTLE_MS = num("SETTLE_MS", 20000);
const RESTART_DEADLINE_MS = num("RESTART_DEADLINE_MS", 15000);
const HOLD_MS = num("HOLD_MS", 20000);
const CONNECT_TIMEOUT_MS = num("CONNECT_TIMEOUT_MS", 120000);
const TURN_HOST = process.env.TURN_HOST;
const TURN_PORT = num("TURN_PORT", 3478);
const servers = [{
  urls: [`turn:${TURN_HOST}:${TURN_PORT}?transport=udp`],
  username: process.env.TURN_USER || "build",
  credential: process.env.TURN_PASSWORD || "soak",
}];

const started = Date.now();
const stamp = () => `${((Date.now() - started) / 1000).toFixed(1).padStart(7)}s`;
const log = (...parts) => console.log(stamp(), ...parts);

/** Relay only, through the stack's coturn, at both ends — and every peer the
 *  app makes kept where this script can reach it. */
async function forceRelay(context) {
  await context.addInitScript((servers) => {
    const Native = window.RTCPeerConnection;
    if (!Native) return;
    window.__peers = [];
    const relayed = (config = {}) => ({ ...config, iceServers: servers, iceTransportPolicy: "relay" });
    window.RTCPeerConnection = function RelayedPeerConnection(config, ...rest) {
      const peer = new Native(relayed(config), ...rest);
      // The app's restart hands the peer fresh servers with
      // `setConfiguration({ iceServers })`, and a configuration set that way
      // takes every member it leaves out back to its default — the policy
      // included, which would let the restarted pair go direct.
      const setConfiguration = peer.setConfiguration.bind(peer);
      peer.setConfiguration = (config) => setConfiguration(relayed(config));
      window.__peers.push(peer);
      return peer;
    };
    window.RTCPeerConnection.prototype = Native.prototype;
  }, servers);
  // The list the app hands the bridge on its own `rtc.offer`, so the bridge
  // allocates through the same coturn.
  await context.route("**/api/rtc/ice-servers", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ iceServers: servers }) }));
}

/** What the app recorded about its connection, oldest first. */
async function diagnostics(page) {
  const report = await page.evaluate(() => window.buildConnectionDiagnostics?.() ?? null);
  const rows = Array.isArray(report) ? report : report?.events;
  if (!Array.isArray(rows)) throw new Error(`buildConnectionDiagnostics() answered ${JSON.stringify(report)}`);
  return rows;
}

async function signIn(page) {
  await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
  await page.fill('input[name="email"]', EMAIL);
  await page.fill('input[name="name"]', "ICE restart check").catch(() => {});
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load", timeout: 15000 }).catch(() => {}),
    page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
  ]);
  await page.goto(`${APP}/app/`, { waitUntil: "load" });
}

/** Wait for `predicate(rows)` to hold, reading the diagnostics every 100 ms. */
async function until(page, predicate, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const rows = await diagnostics(page);
    const found = predicate(rows);
    if (found) return found;
    if (Date.now() > deadline) return null;
    await page.waitForTimeout(100);
  }
}

const isConnected = (row, phase) => row.event === "connected" && (!phase || row.phase === phase);

/** The one synthetic thing: the newest peer reads `failed` for exactly the
 *  moment the app's listener looks, and then reads its own state again. */
const reportFailedOnce = (page) =>
  page.evaluate(() => {
    const peer = window.__peers.at(-1);
    Object.defineProperty(peer, "connectionState", { get: () => "failed", configurable: true });
    peer.dispatchEvent(new Event("connectionstatechange"));
    delete peer.connectionState;
    return window.__peers.length;
  });

async function main() {
  if (!TURN_HOST) throw new Error("TURN_HOST names the stack's coturn (an address both ends can reach)");
  const browser = await chromium.launch({
    headless: true,
    executablePath: process.env.CHROMIUM_PATH || undefined,
    args: ["--no-sandbox"],
  });
  try {
    // The dev app's CSP names the default stack's relay port (18090) and not
    // this stack's (18128, deploy/compose.liveness.yml); the page is otherwise
    // the product's own.
    const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, bypassCSP: true });
    await forceRelay(context);
    const page = await context.newPage();
    page.on("pageerror", (error) => log("[pageerror]", error.message));
    page.on("console", (message) => {
      if (["error", "warning"].includes(message.type())) log(`[console.${message.type()}]`, message.text().slice(0, 300));
    });
    page.on("requestfailed", (request) => log("[requestfailed]", request.url(), request.failure()?.errorText));
    await signIn(page);

    const opened = await until(page, (rows) => rows.find((row) => isConnected(row)), CONNECT_TIMEOUT_MS);
    if (!opened) {
      log("never connected:", JSON.stringify((await diagnostics(page)).slice(-40)));
      log("page says:", (await page.evaluate(() => document.body.innerText)).slice(0, 600).replace(/\s+/g, " "));
      return 2;
    }
    log(`connected; settling ${SETTLE_MS} ms before the restart`);
    await page.waitForTimeout(SETTLE_MS);

    // By the page's own clock, not by position: the history is a bounded
    // ring, and a row that fell off its end would shift every index after it.
    const since = await page.evaluate(() => Date.now());
    const after = (rows) => rows.filter((row) => row.at >= since);
    const peers = await reportFailedOnce(page);
    log(`the newest of ${peers} peer(s) reported failed; the app restarts ICE`);
    const outcome = await until(
      page,
      (rows) => {
        const recent = after(rows);
        const restarting = recent.find((row) => row.event === "restarting");
        const landed = recent.find((row) => isConnected(row, "restart"));
        const failed = recent.find((row) => row.event === "restart-failed");
        return (landed || failed) && { restarting, landed, failed };
      },
      RESTART_DEADLINE_MS + 15000,
    );
    const rows = after(await diagnostics(page));
    const carried = rows.filter((row) => row.event === "carrying").map((row) => row.path);
    for (const row of rows.filter((row) => row.event !== "state")) {
      const { at, connection, event, ...rest } = row;
      log(`  ${new Date(at).toISOString().slice(11, 23)} ${event} ${JSON.stringify(rest)}`);
    }
    if (!outcome?.restarting || !outcome.landed) {
      log(`RESULT FAIL: the restart did not land${outcome?.failed ? ` (${JSON.stringify(outcome.failed)})` : " at all"}`);
      return 1;
    }
    const tookMs = outcome.landed.at - outcome.restarting.at;
    log(`restart landed in ${tookMs} ms over ${carried.at(-1) || "?"}`);
    if (tookMs > RESTART_DEADLINE_MS || carried.at(-1) !== "turn") {
      log(`RESULT FAIL: over ${RESTART_DEADLINE_MS} ms, or not on the relayed path`);
      return 1;
    }

    await page.waitForTimeout(HOLD_MS);
    const lost = after(await diagnostics(page)).filter((row) => ["restart-failed", "closed"].includes(row.event));
    if (lost.length > 0) {
      log(`RESULT FAIL: the restarted session did not hold: ${JSON.stringify(lost)}`);
      return 1;
    }
    log(`RESULT PASS: restarted in ${tookMs} ms on a relayed path and held ${HOLD_MS} ms`);
    return 0;
  } finally {
    await browser.close();
  }
}

main().then(
  (code) => process.exit(code),
  (error) => {
    console.error(`ice-restart-check: ${error.stack || error.message}`);
    process.exit(2);
  },
);
