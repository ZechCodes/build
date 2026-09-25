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
//      Diagnostics shows) time it: `restarting` to the restart's `connected`;
//   4. the peer itself says it was a restart, not the old path carrying on
//      under a new name: within the same deadline, the same RTCPeerConnection
//      reaches a new ICE generation at both ends (a new ufrag in the local and
//      in the bridge's description) with a selected pair that is not the old
//      one, succeeded, nominated and relayed. The synthetic `failed` leaves
//      the old path working, so the app's word alone cannot tell a restart
//      from none (#131 review: with `iceRestart` stripped from the offer, the
//      ufrags stayed and the diagnostics still said "restarted").
//      NEGATIVE_CONTROL=1 strips it that way, and must fail;
//   5. and the new pair carries the session, asked rather than watched: an
//      idle path carries nothing, so once the generation is selected, and
//      every PROBE_EVERY_MS through the hold, the check has the app send —
//      `buildConnectionProbe()`, the app's own carry check (a `ping` over the
//      path its session rides) — and requires every machine answered and the
//      exchange counted on the new pair both ways.
//
// Exit 0 when the restart landed within RESTART_DEADLINE_MS on a relayed path
// of a new ICE generation that carried every probe for HOLD_MS after; 1 when
// it did not; 2 when the run could not get as far as a restart (no connection
// to begin with).
//
// It runs inside mcr.microsoft.com/playwright with the host's network, so it
// reaches the stack's published ports and coturn's address on the compose
// network (scripts/liveness-gate.sh starts it that way):
//
//   APP=http://localhost:8128  TURN_HOST=<coturn IP>  TURN_USER / TURN_PASSWORD
//   SETTLE_MS=20000   how long the session carries before the restart
//   RESTART_DEADLINE_MS=15000   HOLD_MS=20000   PROBE_EVERY_MS=4000
//   CHROMIUM_PATH (optional)
//   NEGATIVE_CONTROL=1   the app's restart offer loses `iceRestart`

import { chromium } from "playwright";

const num = (name, fallback) => Number(process.env[name] || fallback);
const APP = process.env.APP || "http://localhost:8128";
const EMAIL = process.env.QA_EMAIL || "qa@localhost";
const SETTLE_MS = num("SETTLE_MS", 20000);
const RESTART_DEADLINE_MS = num("RESTART_DEADLINE_MS", 15000);
const HOLD_MS = num("HOLD_MS", 20000);
const PROBE_EVERY_MS = num("PROBE_EVERY_MS", 4000);
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

/** The app's restart offer, stripped of `iceRestart`: what the check must
 *  catch — the app says it restarted, and ICE never did. */
async function stripIceRestart(context) {
  await context.addInitScript(() => {
    const createOffer = RTCPeerConnection.prototype.createOffer;
    RTCPeerConnection.prototype.createOffer = function (options, ...rest) {
      return createOffer.call(this, options && { ...options, iceRestart: false }, ...rest);
    };
  });
}

/** Where the newest peer's ICE stands: the ufrag of each end's description,
 *  and the candidate pair its transport has selected, with the pair's
 *  counters and the local candidate's type. */
const iceOf = (page) =>
  page.evaluate(async () => {
    const peer = window.__peers.at(-1);
    const ufrag = (description) => description?.sdp.match(/a=ice-ufrag:(\S+)/)?.[1] ?? null;
    const reports = [...(await peer.getStats()).values()];
    const transport = reports.find((report) => report.type === "transport" && report.selectedCandidatePairId);
    const pair = reports.find((report) => report.id === transport?.selectedCandidatePairId);
    const local = reports.find((report) => report.id === pair?.localCandidateId);
    return {
      peers: window.__peers.length,
      local: ufrag(peer.localDescription),
      remote: ufrag(peer.remoteDescription),
      pair: pair && {
        id: pair.id,
        state: pair.state,
        nominated: pair.nominated,
        bytesSent: pair.bytesSent,
        bytesReceived: pair.bytesReceived,
        localType: local?.candidateType,
      },
    };
  });

/** Why `after` is not a new ICE generation of `before`, selected on a
 *  relayed pair, or null when it is. */
function notARestart(before, after) {
  if (after.peers !== before.peers) return `a new connection (${before.peers} → ${after.peers} peers), not an ICE restart`;
  if (!after.local || after.local === before.local) return `the local ufrag did not change (${before.local} → ${after.local})`;
  if (!after.remote || after.remote === before.remote) return `the bridge's ufrag did not change (${before.remote} → ${after.remote})`;
  const pair = after.pair;
  if (!pair) return "no candidate pair is selected";
  if (pair.id === before.pair?.id) return `the old candidate pair ${pair.id} is still the selected one`;
  if (pair.state !== "succeeded" || !pair.nominated) return `the new pair is ${pair.state}, nominated ${pair.nominated}`;
  if (pair.localType !== "relay") return `the new pair is ${pair.localType}, not relayed`;
  return null;
}

/** Wait, until `deadline` (this process's clock), for the newest peer to
 *  reach a new generation of `before`. `{ ice }` when it did; `{ why, ice }`
 *  with the last reason when the deadline passed first. */
async function newGeneration(page, before, deadline) {
  for (;;) {
    const ice = await iceOf(page);
    const why = notARestart(before, ice);
    if (!why) return { ice };
    if (Date.now() > deadline) return { why, ice };
    await page.waitForTimeout(100);
  }
}

/** Have the app carry something now — `buildConnectionProbe()`, one carry
 *  check per machine over the path its session rides — and require every
 *  machine to answer with the exchange counted on `pairId` both ways. `{ why }`
 *  when it did not; `{ ms, sent, received }` when it did. */
async function carriesOn(page, pairId) {
  const before = await iceOf(page);
  const rows = await page.evaluate(() => window.buildConnectionProbe?.(5000) ?? null);
  // Chromium serves getStats from a cache a few tens of milliseconds old.
  await page.waitForTimeout(150);
  const after = await iceOf(page);
  if (!Array.isArray(rows)) return { why: "this app has no buildConnectionProbe() to ask" };
  if (rows.length === 0 || !rows.every((row) => row.carried)) return { why: `the app's probe went unanswered: ${JSON.stringify(rows)}` };
  if (before.pair?.id !== pairId || after.pair?.id !== pairId) {
    return { why: `the selected pair moved off ${pairId} (${before.pair?.id} → ${after.pair?.id})` };
  }
  const sent = after.pair.bytesSent - before.pair.bytesSent;
  const received = after.pair.bytesReceived - before.pair.bytesReceived;
  if (!(sent > 0 && received > 0)) return { why: `the probe did not cross ${pairId}: ${sent} bytes out, ${received} in` };
  return { ms: Math.max(...rows.map((row) => row.ms)), sent, received };
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
    if (process.env.NEGATIVE_CONTROL === "1") {
      log("NEGATIVE CONTROL: the restart offer loses iceRestart; this run must fail");
      await stripIceRestart(context);
    }
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
    const before = await iceOf(page);
    log(`before: ${JSON.stringify(before)}`);
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

    // The new generation, within the same deadline the landing had.
    const pageNow = await page.evaluate(() => Date.now());
    const deadline = Date.now() + RESTART_DEADLINE_MS - (pageNow - outcome.restarting.at);
    const reached = await newGeneration(page, before, deadline);
    log(`after: ${JSON.stringify(reached.ice)}`);
    if (reached.why) {
      log(`RESULT FAIL: the app said it restarted, and ICE did not within ${RESTART_DEADLINE_MS} ms: ${reached.why}`);
      return 1;
    }
    const restarted = reached.ice;
    const generationMs = (await page.evaluate(() => Date.now())) - outcome.restarting.at;
    log(`new generation ${restarted.local}/${restarted.remote} on ${restarted.pair.id}, ${generationMs} ms after restarting`);

    // Carried, asked: once now, and every PROBE_EVERY_MS through the hold.
    const holdEnds = Date.now() + HOLD_MS;
    const probes = [];
    for (;;) {
      const probe = await carriesOn(page, restarted.pair.id);
      if (probe.why) {
        log(`RESULT FAIL: probe ${probes.length + 1} on the restarted path: ${probe.why}`);
        return 1;
      }
      probes.push(probe);
      log(`  probe ${probes.length}: answered in ${probe.ms} ms, ${probe.sent} bytes out and ${probe.received} in on ${restarted.pair.id}`);
      if (Date.now() >= holdEnds) break;
      await page.waitForTimeout(Math.min(PROBE_EVERY_MS, Math.max(0, holdEnds - Date.now())));
    }
    const lost = after(await diagnostics(page)).filter((row) => ["restart-failed", "closed"].includes(row.event));
    if (lost.length > 0) {
      log(`RESULT FAIL: the restarted session did not hold: ${JSON.stringify(lost)}`);
      return 1;
    }
    log(
      `RESULT PASS: restarted in ${tookMs} ms onto ICE generation ${restarted.local}/${restarted.remote} ` +
        `(was ${before.local}/${before.remote}), relayed, and carried all ${probes.length} probes over ${HOLD_MS} ms`,
    );
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
