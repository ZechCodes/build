// #31 on a real stack: the hold makes a direct pair win a race a relay pair
// would otherwise have won.
//
//   TASKS_REPO=<this checkout> node web/relay-wins-check.mjs
//
// Reads /tmp/live-seed.json, so run web/live-seed.mjs first and write its SEED
// line there. Brings coturn up and takes it down again; touches nothing else
// about the stack.
//
// Exit 0 the hold won the race, 1 it did not, 3 this machine cannot host the
// experiment at all (the control could not hand the race to the relay pair).
//
// # Why the measurement in #31 could not settle anything
//
// The compose stack's ICE servers carry no TURN at all, so a browser on it
// gathers no relay candidate: there is no race to win or lose, and ten sessions
// before and after the change both came up direct. That is a no-regression
// result and nothing more. `relayed-path.mjs` goes the other way and forces
// `iceTransportPolicy: "relay"`, which proves TURN still works but removes the
// race in the opposite direction — with no direct candidate there is again
// nothing to choose between.
//
// This harness builds the race itself: both kinds of candidate present, and the
// direct ones arriving LATE, which is the condition the maintainer's Wi-Fi
// produces and this machine does not.
//
// # Why it once exited 3 here (#55)
//
// coturn used to run with `--net=host` and be handed out at the compose gateway.
// The browser reached it — it runs on the host — but this host's firewall drops
// inbound traffic from containers by default, so every Allocate the bridge sent
// was dropped and the bridge gathered only host candidates. With one remote
// candidate kind, a direct and a relayed pair became available at the same
// instant and no skew could bias the race. coturn now sits on the compose
// network, the bridge logs `gathered host=1 relay=1`, and the control hands the
// race to the relay pair as designed. An exit 3 again most likely means the
// bridge's own relay candidate is missing: read its `rtc: … gathered` line and
// any `rtc: WARN|ERROR` line from webrtc's TURN client in bridge.err.log.
//
// # The experiment, and its control
//
// A coturn on the compose network so both ends really gather relay candidates
// (the bridge's own `rtc:` line reads `gathered … relay=N`), and an init
// script that delays every non-relay candidate on its way into
// `addIceCandidate` — the browser's own check list is where nomination is
// decided, so that is where the skew has to go.
//
// Two phases, and the control is what makes the result mean anything:
//
//   • skew LONGER than the hold window (2.5 s vs 1.5 s) — the relay pair should
//     win. If it does not, the skew is not biasing the race and the other phase
//     proves nothing, because "direct won" would have happened anyway.
//   • skew SHORTER than the hold window (0.8 s) — the relay pair arrives first
//     and is held; the direct pair arrives during the window and wins.
//
// Same bundle, same stack, one variable. That is the difference between showing
// the hold changes an outcome and asserting it.
//
// A third phase watches one relayed session past the twenty-second mark, where
// the one re-nomination attempt lives: a relay session with a direct pair that
// has since succeeded should move itself onto it.

import { existsSync, readFileSync } from "node:fs";
import { execSync } from "node:child_process";
import { chromium } from "playwright";

const APP = process.env.APP_URL || "http://localhost:8090";
const CHROMIUM = "/usr/bin/chromium";
const RUNS = Number(process.env.RUNS || 4);

const REPO = process.env.TASKS_REPO;
if (!REPO) {
  console.error("set TASKS_REPO to the checkout under test — this run must not mix checkouts");
  process.exit(2);
}
if (!existsSync(`${REPO}/deploy/compose.real.yml`)) {
  console.error(`no compose file under ${REPO} — is TASKS_REPO a build-web checkout?`);
  process.exit(2);
}

/** The hold window and the attempt delay, read from the modules that arm them so
 *  the two skews this run picks cannot end up on the wrong side of them. */
const timing = (() => {
  const ice = readFileSync(`${REPO}/spa/src/core/iceCandidates.js`, "utf8");
  const peer = readFileSync(`${REPO}/spa/src/core/peerLink.js`, "utf8");
  const read = (name, source, pattern) => {
    const found = source.match(pattern);
    if (!found) throw new Error(`no ${name} in the SPA — this check needs it`);
    return Number(found[1]);
  };
  return {
    holdMs: read("RELAY_HOLD_MS", ice, /RELAY_HOLD_MS\s*=\s*(\d+)/),
    upgradeMs: read("RELAY_UPGRADE_AFTER_MS", peer, /RELAY_UPGRADE_AFTER_MS\s*=\s*(\d+)/),
  };
})();
/** Inside the window: the direct pair arrives while the relay one is held. */
const SKEW_HELD_MS = Math.round(timing.holdMs * 0.55);
/** Past the window: the relay candidate is released before the direct one turns
 *  up at all, so the relay pair wins. The control. */
const SKEW_PAST_MS = Math.round(timing.holdMs * 1.7);

const seed = JSON.parse(readFileSync("/tmp/live-seed.json", "utf8"));
const workspace = seed.workspaces[0];
const docker = (command, { quiet = false } = {}) => {
  try {
    return execSync(`echo '${command}' | newgrp docker 2>&1`, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024, shell: "/bin/bash" });
  } catch (error) {
    if (!quiet) throw error;
    return String(error.stdout || "");
  }
};

const results = [];
const record = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
};

// ── coturn, so both ends really gather relay candidates ─────────────────────
const TURN = { container: "build-relay-wins-turn", port: 3478, user: "build", password: "relay-wins", realm: "build.test" };
/** The address BOTH ends can reach coturn at: its own address on the compose
 *  network. The bridge sits on that network, and the host routes to it over the
 *  docker bridge interface, so neither end's packets cross the host's INPUT
 *  firewall. The compose gateway looks like the same thing and is not: on a
 *  host whose firewall drops inbound traffic by default (ufw here), the browser
 *  reaches the gateway because it IS the host, and every Allocate the bridge
 *  sends there is dropped — the bridge then gathers no relay candidate and the
 *  race below has only one kind of remote candidate in it (#55). */
const COMPOSE_NETWORK = process.env.COMPOSE_NETWORK || "deploy_default";
let turnAddress = null;
const turnHost = () => {
  if (process.env.TURN_HOST) return process.env.TURN_HOST;
  if (!turnAddress) {
    // Parsed from the JSON rather than asked for with a template: `docker()`
    // wraps the whole command in single quotes, which a template cannot survive.
    const [inspected] = JSON.parse(docker(`docker inspect ${TURN.container}`));
    turnAddress = inspected?.NetworkSettings?.Networks?.[COMPOSE_NETWORK]?.IPAddress ?? "";
  }
  if (!/^[0-9.]+$/.test(turnAddress)) throw new Error(`coturn has no address on ${COMPOSE_NETWORK}: ${turnAddress}`);
  return turnAddress;
};
const iceServers = () => [
  { urls: [`turn:${turnHost()}:${TURN.port}?transport=udp`], username: TURN.user, credential: TURN.password },
];

let turnUp = false;
const stopTurn = () => {
  if (!turnUp) return;
  turnUp = false;
  docker(`docker rm -f ${TURN.container}`, { quiet: true });
};
/** Taken down on any exit: a coturn left running on the host is a port somebody
 *  else's run will find occupied. */
for (const signal of ["exit", "SIGINT", "SIGTERM"]) process.on(signal, stopTurn);

const startTurn = () => {
  docker(`docker rm -f ${TURN.container}`, { quiet: true });
  // On the compose network, so the relay addresses it hands out are its own
  // address there — reachable from the bridge beside it and from the host's
  // browser alike (see turnHost).
  turnAddress = null;
  docker(
    `docker run -d --name ${TURN.container} --network ${COMPOSE_NETWORK} coturn/coturn:latest ` +
      `-n --listening-port=${TURN.port} --fingerprint --lt-cred-mech ` +
      `--user=${TURN.user}:${TURN.password} --realm=${TURN.realm} --no-tls --no-cli --log-file=stdout`,
  );
  execSync("sleep 2");
  if (docker(`docker inspect -f '{{.State.Running}}' ${TURN.container}`).trim() !== "true") {
    throw new Error(`coturn did not start: ${docker(`docker logs ${TURN.container}`, { quiet: true }).slice(-300)}`);
  }
  turnUp = true;
  console.log(`coturn up on turn:${turnHost()}:${TURN.port} (user ${TURN.user})`);
};

/**
 * Make every direct candidate late, in BOTH directions.
 *
 * The exact mirror of the product's hold: it delays RELAY candidates so a direct
 * pair can win, and this delays DIRECT ones so a relay pair can. What it
 * simulates is a network where a host pair waits on mDNS resolution and consent
 * while a TURN allocation is ready at once — a user's, and not this machine's,
 * where every candidate is a plain address on one docker bridge.
 *
 * Both directions, because one is not enough and the first run of this harness
 * proved it: delaying only the candidates arriving at `addIceCandidate` left the
 * bridge free to learn the browser's host address and send its own connectivity
 * checks to it, and an incoming check from an unknown address makes a
 * peer-reflexive candidate and a valid direct pair with no signalled candidate
 * involved at all. So the browser's own host candidates are held on the way OUT
 * too — intercepted at the `icecandidate` event, before the product's listener
 * sees them — and then neither end can form a direct pair until the window the
 * skew names.
 *
 * Nothing about the product is changed; both hooks are installed before any of
 * its code runs.
 */
const skewDirectCandidatesLate = (context, delayMs) =>
  context.addInitScript((delay) => {
    const Native = window.RTCPeerConnection;
    if (!Native) return;
    const isRelay = (line) => / typ relay(\s|$)/.test(line || "");

    // Inbound: the bridge's candidates, on their way into this check list.
    const addIceCandidate = Native.prototype.addIceCandidate;
    Native.prototype.addIceCandidate = function skewedInbound(candidate, ...rest) {
      const line = (candidate && (candidate.candidate ?? "")) || "";
      if (isRelay(line) || !line) return addIceCandidate.call(this, candidate, ...rest);
      return new Promise((resolve, reject) => {
        setTimeout(() => addIceCandidate.call(this, candidate, ...rest).then(resolve, reject), delay);
      });
    };

    // Outbound: our own candidates, before whoever is listening can signal them.
    const addEventListener = Native.prototype.addEventListener;
    Native.prototype.addEventListener = function skewedOutbound(type, listener, ...rest) {
      if (type !== "icecandidate" || typeof listener !== "function") {
        return addEventListener.call(this, type, listener, ...rest);
      }
      const held = (event) => {
        const line = event?.candidate?.candidate || "";
        if (!event?.candidate || isRelay(line)) return listener.call(this, event);
        // The event object is not reused by the agent once dispatched, so it is
        // safe to hand the same one over late.
        setTimeout(() => listener.call(this, event), delay);
        return undefined;
      };
      return addEventListener.call(this, type, held, ...rest);
    };

    window.__skewedDirectBy = delay;
  }, delayMs);

/** One run's browser, with the skew and the TURN list installed. */
async function openBrowser(delayMs) {
  const browser = await chromium.launch({ executablePath: CHROMIUM, headless: true, args: ["--no-sandbox"] });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  await skewDirectCandidatesLate(context, delayMs);
  // The list the app hands the bridge, so both ends allocate through coturn: the
  // client sends this array on its own `rtc.offer`, and the bridge answers from
  // it, so the one route relays both ends.
  await context.route("**/api/rtc/ice-servers", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ iceServers: iceServers() }) }));
  const page = await context.newPage();
  await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
  await page.fill('input[name="email"]', "qa@localhost");
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load" }),
    page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
  ]);
  return { browser, page };
}

const route = `${APP}/app/#/project/${seed.projectId}/workspace/${workspace.workspaceId}`;
const carryingNow = (page) =>
  page.evaluate(() => {
    // The report is `{ since, dropped, events }` since #60; a bare array before.
    const report = globalThis.buildConnectionDiagnostics?.();
    const history = (Array.isArray(report) ? report : report?.events) || [];
    return history.filter((entry) => entry.event === "carrying").map((entry) => entry.path).pop() ?? null;
  });
const directPairStates = (page) =>
  page.evaluate(() => {
    const report = globalThis.buildConnectionDiagnostics?.();
    const history = (Array.isArray(report) ? report : report?.events) || [];
    return history.filter((e) => e.event === "direct-pair").map((e) => e.state);
  });

/** What one session landed on, under a given skew. */
async function landings(delayMs, runs, label) {
  const paths = [];
  const { browser, page } = await openBrowser(delayMs);
  try {
    for (let run = 0; run < runs; run++) {
      await page.goto(route, { waitUntil: "load" });
      let path = null;
      for (let step = 0; step < 40 && path === null; step++) {
        await page.waitForTimeout(500);
        path = await carryingNow(page);
      }
      paths.push(path ?? "unknown");
      console.log(`  ${label} ${String(run + 1).padStart(2)}  carrying=${path ?? "unknown"}`);
    }
  } finally {
    await browser.close().catch(() => {});
  }
  return paths;
}

const tally = (paths) => paths.reduce((all, one) => ({ ...all, [one]: (all[one] || 0) + 1 }), {});

startTurn();

// ── the control: a skew past the hold window, where the relay pair should win ─
console.log(`\n── control: direct candidates ${SKEW_PAST_MS} ms late, past the ${timing.holdMs} ms hold ──`);
const control = await landings(SKEW_PAST_MS, RUNS, "control");
const relayWon = control.filter((path) => path === "turn").length;
if (relayWon === 0) {
  // Not a failure of the product, and not something later phases can be read
  // over: if the skew cannot hand the race to the relay pair, "direct won" in the
  // other phase would have happened anyway. Said in its own exit code so a CI
  // run can tell "this machine cannot host the experiment" from "the hold is
  // broken".
  console.log(`\nINCONCLUSIVE  the skew did not hand the race to the relay pair: ${JSON.stringify(tally(control))}`);
  console.log("  Most likely the bridge gathered no relay candidate of its own: check its");
  console.log("  `rtc: … gathered host=… relay=…` line and any `rtc: WARN`/`ERROR` line from the");
  console.log("  TURN client in the bridge's stderr. See the header (#55).");
  stopTurn();
  process.exit(3);
}
record(
  "the skew really does hand the race to the relay pair",
  relayWon > 0,
  `${JSON.stringify(tally(control))} — without this the other phase would prove nothing`,
);

// ── the test: a skew inside the window, where the hold should win it back ────
console.log(`\n── held: direct candidates ${SKEW_HELD_MS} ms late, inside the ${timing.holdMs} ms hold ──`);
const held = await landings(SKEW_HELD_MS, RUNS, "held");
const directWon = held.filter((path) => path === "direct").length;
record(
  "the hold wins the same race back for the direct pair",
  directWon === held.length,
  `${JSON.stringify(tally(held))} of ${held.length}`,
);
const directPastWindow = control.filter((path) => path === "direct").length;
record(
  "…and it is the hold doing it, not the machine",
  directWon > directPastWindow,
  `direct ${directWon}/${held.length} inside the window vs ${directPastWindow}/${control.length} past it`,
);

// ── the one re-nomination attempt, on a session that did land on relay ──────
console.log(`\n── a relayed session, watched past ${Math.round(timing.upgradeMs / 1000)}s ──`);
const { browser, page } = await openBrowser(SKEW_PAST_MS);
try {
  await page.goto(route, { waitUntil: "load" });
  let landed = null;
  for (let step = 0; step < 40 && landed === null; step++) {
    await page.waitForTimeout(500);
    landed = await carryingNow(page);
  }
  record("the session lands on relay, as the control said it would", landed === "turn", `carrying=${landed}`);
  let states = [];
  for (let step = 0; step < Math.ceil(timing.upgradeMs / 1000) + 20 && !states.length; step++) {
    await page.waitForTimeout(1000);
    states = await directPairStates(page);
  }
  record(
    "the one attempt at a direct pair runs once the session is steady",
    states.includes("trying") || states.includes("none-to-try"),
    `direct-pair ${JSON.stringify(states)}`,
  );
  if (states.includes("trying")) {
    for (let step = 0; step < 30 && !states.some((one) => ["renominated", "stayed-relayed", "failed"].includes(one)); step++) {
      await page.waitForTimeout(1000);
      states = await directPairStates(page);
    }
    const settled = states.find((one) => ["renominated", "stayed-relayed", "failed"].includes(one));
    record(
      "…and it settles, without costing the session either way",
      Boolean(settled) && (await carryingNow(page)) !== null,
      `${JSON.stringify(states)}, now carrying=${await carryingNow(page)}`,
    );
  }
} finally {
  await browser.close().catch(() => {});
}

stopTurn();
console.log("\n──────── summary ────────");
for (const { name, ok, detail } of results) console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
const passed = results.filter((one) => one.ok).length;
console.log(`\n${passed}/${results.length} passed`);
process.exit(passed === results.length ? 0 : 1);
