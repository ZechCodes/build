// The phone's conditions, locally: every peer connection forced through a TURN
// relay.
//
// Zech's phone carries `host/relay` on every session and closes its peer every
// six to twelve seconds; the compose stack pairs `host/host` and does not. That
// difference is the one thing the reproduction was missing, and this is how it
// is removed — without changing a line of the product.
//
// Two levers, both in the browser:
//
//   1. the ICE-servers route is fulfilled with a coturn this script runs, so
//      both ends allocate through it (the client's list rides its own
//      `rtc.offer`, so the bridge follows automatically);
//   2. `RTCPeerConnection` is patched in an init script to force
//      `iceTransportPolicy: "relay"`, so no host or reflexive pair can win.
//
// It then runs the two things worth watching over that path: an idle session,
// and a bridge that stops answering while ICE holds.
//
// Usage (host, from Build/web):  node relayed-path.mjs
//   IDLE_MS=90000   how long the idle watch runs
//   PAUSE_S=9       how long the bridge is paused in the wedge watch
//   KEEP_TURN=1     leave coturn running afterwards (for a hand-driven look)
//
// It brings coturn up and takes it down again. It does NOT touch the compose
// stack: that has to be up already, paired and seeded, with /tmp/live-seed.json
// naming the workspaces (deploy/compose.real.yml, web/pair.mjs, web/live-seed.mjs).

import { readFileSync } from "node:fs";
import { execSync } from "node:child_process";
import { chromium } from "/home/zech/.local/share/mise/installs/npm-playwright/1.63.0/node_modules/playwright/index.mjs";

const APP = process.env.APP || "http://localhost:8090";
const EMAIL = process.env.QA_EMAIL || "qa@localhost";
const COMPOSE = process.env.COMPOSE || `${process.cwd().replace(/\/web$/, "")}/deploy/compose.real.yml`;
const IDLE_MS = Number(process.env.IDLE_MS || 90000);
const PAUSE_S = Number(process.env.PAUSE_S || 9);
const BRIDGE = process.env.BRIDGE_CONTAINER || "deploy-bridge-1";

const docker = (command, { quiet = false } = {}) => {
  try {
    return execSync(`echo '${command}' | newgrp docker`, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  } catch (error) {
    if (!quiet) throw error;
    return String(error.stdout || "");
  }
};

/** The relay this run forces every candidate through. Host networking, because
 *  both ends have to reach it under the same address: the browser runs on the
 *  host and the bridge runs in a container that reaches the host's 3478. */
const TURN = {
  container: "build-relayed-path-turn",
  port: 3478,
  user: "build",
  password: "relayed-path",
  realm: "build.test",
};
/** The address BOTH ends can reach coturn at.
 *
 *  Not `127.0.0.1`: the browser runs on the host and the bridge runs in a
 *  container, where that name is the container itself. The compose network's
 *  gateway is the host as the container sees it, and the host answers on it
 *  too, so one URL serves both — which it has to, because the client's list is
 *  what the bridge is handed. */
const turnHost = () => {
  if (process.env.TURN_HOST) return process.env.TURN_HOST;
  // Read as JSON rather than through a Go template: the template braces do not
  // survive the quoting this has to go through to reach the docker group.
  const network = process.env.COMPOSE_NETWORK || "deploy_default";
  try {
    const [inspected] = JSON.parse(docker(`docker network inspect ${network}`));
    const gateway = (inspected?.IPAM?.Config || []).map((entry) => entry.Gateway).find(Boolean);
    if (gateway) return gateway;
  } catch {
    /* fall through to docker's default bridge */
  }
  return "172.17.0.1";
};
const turnUrl = () => `turn:${turnHost()}:${TURN.port}?transport=udp`;
const iceServers = () => [{ urls: [turnUrl()], username: TURN.user, credential: TURN.password }];

const bridgeLog = () => docker(`docker compose -f ${COMPOSE} logs --no-color --tail 4000 bridge`);
const closes = (log) => (log.match(/session_ended_closing_peer/g) || []).length;

function startTurn() {
  docker(`docker rm -f ${TURN.container}`, { quiet: true });
  // `--net=host` so the relayed candidates it hands out are the host's own
  // address: a bridged container would advertise an address the browser
  // cannot reach, and every pair would fail instead of relaying.
  docker(
    `docker run -d --name ${TURN.container} --net=host coturn/coturn:latest ` +
      `-n --listening-port=${TURN.port} --fingerprint --lt-cred-mech ` +
      `--user=${TURN.user}:${TURN.password} --realm=${TURN.realm} ` +
      `--no-tls --no-cli --log-file=stdout`,
  );
  // coturn answers on the port a moment after the container starts.
  execSync("sleep 2");
  const state = docker(`docker inspect -f '{{.State.Running}}' ${TURN.container}`).trim();
  if (state !== "true") throw new Error(`coturn did not start: ${docker(`docker logs ${TURN.container}`, { quiet: true })}`);
  console.log(`coturn up on ${turnUrl()} (user ${TURN.user})`);
}

const stopTurn = () => docker(`docker rm -f ${TURN.container}`, { quiet: true });

/** Everything the page needs to be on a relayed path, installed before any of
 *  its own code runs. */
async function forceRelay(context, servers) {
  await context.addInitScript((servers) => {
    const Native = window.RTCPeerConnection;
    if (!Native) return;
    // The product asks for `{ iceServers }`; this run says "relay only, and
    // through these". Everything else about the connection is the product's.
    window.RTCPeerConnection = function RelayedPeerConnection(config = {}, ...rest) {
      return new Native({ ...config, iceServers: servers, iceTransportPolicy: "relay" }, ...rest);
    };
    window.RTCPeerConnection.prototype = Native.prototype;
    window.__relayForced = true;
  }, servers);
  // And the list the app hands the bridge, so both ends allocate through it:
  // the client sends this array on its own `rtc.offer` (core/peerLink.js), and
  // the bridge answers from it, so patching the one route relays both ends.
  await context.route("**/api/rtc/ice-servers", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ iceServers: servers }) }));
}

const diagnostics = (page) => page.evaluate(() => window.buildConnectionDiagnostics?.() || []);

const printRows = (rows, limit = 40) => {
  for (const row of rows.slice(-limit)) {
    const { at, connection, event, ...rest } = row;
    console.log(`  ${new Date(at).toISOString().slice(11, 23)} ${event} ${JSON.stringify(rest)}`);
  }
};

/** What path the browser ended up on, as the app itself measured it. */
const carrying = (rows) => rows.filter((row) => row.event === "carrying").map((row) => row.path);

async function signIn(page, seed) {
  await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
  await page.fill('input[name="email"]', EMAIL);
  await page.fill('input[name="name"]', "Relayed Path");
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load", timeout: 15000 }).catch(() => {}),
    page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
  ]);
  await page.goto(`${APP}/app/`, { waitUntil: "load" });
  const [workspace] = seed.workspaces;
  await page.waitForFunction((name) => document.body.innerText.includes(name), workspace.name, { timeout: 90000 });
  await page.evaluate((hash) => { location.hash = hash; }, `#/project/${seed.projectId}/workspace/${workspace.workspaceId}`);
  await page.waitForTimeout(4000);
}

async function watchIdle(page) {
  console.log(`\n── idle ${IDLE_MS} ms on a relayed path ──`);
  const before = closes(bridgeLog());
  await page.waitForTimeout(IDLE_MS);
  const rows = await diagnostics(page);
  console.log(`peer closes: ${closes(bridgeLog()) - before}   carrying: ${[...new Set(carrying(rows))].join(",") || "?"}`);
  printRows(rows.filter((row) => row.event !== "state"));
}

async function watchWedge(page) {
  console.log(`\n── the bridge stops answering for ${PAUSE_S}s, ICE untouched ──`);
  const before = closes(bridgeLog());
  docker(`docker pause ${BRIDGE}`);
  try {
    await page.waitForTimeout(PAUSE_S * 1000);
  } finally {
    docker(`docker unpause ${BRIDGE}`, { quiet: true });
  }
  await page.waitForTimeout(6000);
  const rows = await diagnostics(page);
  console.log(`peer closes: ${closes(bridgeLog()) - before}`);
  printRows(rows.filter((row) => row.event !== "state"), 24);
}

const seed = JSON.parse(readFileSync("/tmp/live-seed.json", "utf8"));
startTurn();
const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
try {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  await forceRelay(context, iceServers());
  const page = await context.newPage();
  page.on("pageerror", (error) => console.log("  [pageerror]", error.message));
  page.on("console", (message) => { if (message.type() === "error") console.log("  [console.error]", message.text().slice(0, 200)); });

  await signIn(page, seed);
  const opening = await diagnostics(page);
  const path = carrying(opening).at(-1);
  console.log(`forced relay: ${await page.evaluate(() => window.__relayForced === true)}   app says carrying: ${path || "?"}`);
  if (path !== "turn") {
    console.log("!! the connection is not relayed — the rest of this run says nothing about the phone's conditions");
  }

  await watchIdle(page);
  await watchWedge(page);
} finally {
  await browser.close();
  if (!process.env.KEEP_TURN) stopTurn();
}
