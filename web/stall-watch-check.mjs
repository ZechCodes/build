// #41 on a real stack: the bridge notices its own frames are not leaving.
//
//   ISSUES_REPO=<this checkout> node web/stall-watch-check.mjs
//
// Reads /tmp/live-seed.json, so run web/live-seed.mjs first and write its SEED
// line there. Exits non-zero on a failed check.
//
// # Why this exists beside web/dead-path-check.mjs
//
// That check is the BROWSER noticing a dead path, and it reproduces one with
// `docker pause`. This one is the BRIDGE noticing, and a paused bridge cannot
// exercise it by construction: a frozen container executes no code, so the very
// thing under test is not running. The fault has to be made from the other end —
// a client that stops reading while the bridge writes — which is also what
// actually happened to the maintainer: their tablet was gone, the bridge had an
// admission receipt and an `ok` queued for it, and nothing was leaving.
//
// # How a client is made to stop reading
//
// SIGSTOP on the browser's whole process tree. A frozen process does not drain
// its sockets, the kernel receive buffers fill, SCTP stops acknowledging, and the
// bridge's outstanding bytes stop falling — which is exactly the condition the
// watch measures. (A blackhole rule inside the compose network would be the other
// way to do it and needs root and a netns setup this stack does not have.)
//
// The whole tree, not the launched process alone: Chromium spreads its work over
// a browser process and a renderer, and the SCTP association is not read by the
// one Playwright hands back. Stopping by process GROUP is not an option either —
// Playwright does not `setsid`, so the group is this script's own.
//
// # Why a large payload, when a few hundred bytes turn out to be enough
//
// The watch fires on bytes QUEUED AND NOT DRAINING, of which there is no minimum:
// the first run of this harness caught it with `buffered_bytes=622`, which is the
// size of an ordinary reply, and that is the right answer — the maintainer's
// stuck frames were an admission receipt and an `ok`, both small. What the
// watch measures is that nothing is moving, not that a lot is waiting.
//
// So the multi-megabyte attachment is not there to overflow anything. It is there
// to GUARANTEE the bridge has something to write across the moment of the freeze:
// an idle app channel is deliberately not a stall, and without traffic in flight
// the watch would correctly stay silent and this harness would be asserting
// nothing. Whether the stall is then detected on the attachment's own parked
// megabytes or on a smaller frame queued behind it is a matter of timing, and the
// check does not care which.
//
// The attachment is this harness's own — seeded here, not in live-seed.mjs, and
// removed from both of the bridge's attachment homes afterwards.

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { execSync } from "node:child_process";
import { chromium } from "playwright";

const APP = process.env.APP_URL || "http://localhost:8090";
/** The system Chromium, as every browser pass on this machine uses: mise's
 *  Playwright wants a build it has not cached. */
const CHROMIUM = "/usr/bin/chromium";
const BRIDGE = process.env.BRIDGE_CONTAINER || "deploy-bridge-1";

/** The ONE checkout this runs off — refused rather than defaulted, as the other
 *  checks refuse it: there is more than one build-web checkout on this machine
 *  and a forgotten variable would pass against a bridge never under test. */
const REPO = process.env.ISSUES_REPO;
if (!REPO) {
  console.error("set ISSUES_REPO to the checkout under test — this run must not mix checkouts");
  process.exit(2);
}
if (!existsSync(`${REPO}/deploy/compose.real.yml`)) {
  console.error(`no compose file under ${REPO} — is ISSUES_REPO a build-web checkout?`);
  process.exit(2);
}

/** The bridge's own numbers, read from the module that arms them so this script
 *  cannot drift from what it is waiting for. */
const timing = (() => {
  const rtc = readFileSync(`${REPO}/bridge/src/rtc.rs`, "utf8");
  const read = (name, pattern) => {
    const found = rtc.match(pattern);
    if (!found) throw new Error(`no ${name} in bridge/src/rtc.rs — this check needs it`);
    // The buffer limit is written as a product (`1024 * 1024`), so the whole
    // expression is captured and multiplied out. Matching only the first number
    // read it as 1 KiB and would have made the "bytes were queued" check pass on
    // any value at all.
    return found[1]
      .split("*")
      .map((factor) => Number(factor.trim()))
      .reduce((product, factor) => product * factor, 1);
  };
  return {
    stallSeconds: read("DC_STALL_TIMEOUT", /DC_STALL_TIMEOUT: Duration = Duration::from_secs\(([\d\s*]+)\)/),
    pollSeconds: read("DC_STALL_POLL", /DC_STALL_POLL: Duration = Duration::from_secs\(([\d\s*]+)\)/),
    bufferedHigh: read("DC_BUFFERED_HIGH", /DC_BUFFERED_HIGH: usize = ([\d\s*]+);/),
  };
})();
/** How long the freeze lasts: past the stall timeout, plus a poll and a margin,
 *  so a watch that is going to fire has fired. */
const FREEZE_MS = (timing.stallSeconds + timing.pollSeconds + 6) * 1000;
/** What the old behaviour cost — SCTP giving up on its own retransmits. The
 *  whole point is beating this by a wide margin. */
const SCTP_GAVE_UP_AFTER_MS = 105000;

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

/** One bridge RPC from inside the qa container, where the compose network and
 *  the harness deps are. `--no-deps` so this cannot recreate the app or bridge. */
function call(method, params) {
  const script = `import * as transport from "@build/secure-transport"; import { openDeviceLink, openRendezvous } from "./client.mjs"; import { loginWithDummy } from "./skrift-auth.mjs";
const { cookie, mintGatewayToken } = await loginWithDummy(process.env.API_URL, { email: "qa@localhost" });
const rendezvous = await openRendezvous({ relayUrl: process.env.RELAY_URL, mintGatewayToken });
const link = await openDeviceLink({ rendezvous, transport, apiUrl: process.env.API_URL, cookie });
const r = await link.session.call(${JSON.stringify(method)}, ${JSON.stringify(params)});
console.log("RESULT " + JSON.stringify(r)); process.exit(0);`;
  writeFileSync("/tmp/stall-watch-call.mjs", script);
  const out = docker(
    `docker compose -f ${REPO}/deploy/compose.real.yml --profile qa run --rm -T --no-deps qa node --input-type=module - < /tmp/stall-watch-call.mjs`,
  );
  const line = out.split("\n").find((one) => one.startsWith("RESULT "));
  if (!line) throw new Error(`no result from ${method}: ${out.trim().slice(-300)}`);
  return JSON.parse(line.slice("RESULT ".length));
}

/** The bridge's log since this run started, so nothing an earlier session wrote
 *  is mistaken for this one's. */
const startedAt = new Date();
const bridgeLog = () => docker(`docker logs --since ${Math.floor(startedAt.getTime() / 1000)} ${BRIDGE}`, { quiet: true });

// ── seed this harness's own attachment ──────────────────────────────────────
//
// Just under the bridge's 5 MiB cap, and incompressible, so the reply cannot be
// squeezed below the send-buffer limit by anything in the path.
const PAYLOAD_BYTES = 4 * 1024 * 1024;
const payload = Buffer.alloc(PAYLOAD_BYTES);
for (let at = 0; at < PAYLOAD_BYTES; at += 4) payload.writeUInt32LE((at * 2654435761) >>> 0, at);
const stamp = Date.now().toString(36);

console.log(`seeding a ${(PAYLOAD_BYTES / 1048576).toFixed(0)} MiB attachment`);
const uploaded = call("thread.attach", {
  entity_id: workspace.entityId,
  filename: `stall-watch-${stamp}.bin`,
  content_b64: payload.toString("base64"),
});
call("thread.post", {
  entity_id: workspace.entityId,
  agent_id: workspace.agentId,
  conversation_id: workspace.conversationId,
  operation_id: `stall-watch-${stamp}`,
  body: `A large file for web/stall-watch-check.mjs (${stamp}).`,
  attachments: [{ path: uploaded.path, name: uploaded.name }],
});

/** Take this harness's file out of both of the bridge's attachment homes. The
 *  message stays in the transcript — it is a record of what happened — but the
 *  four megabytes do not have to. */
const removeSeededFile = () => {
  const leaf = uploaded.path.split("/").pop();
  if (!/^[A-Za-z0-9._-]+$/.test(leaf)) return; // never interpolate a name we cannot vouch for
  // Both homes, found rather than guessed: `attachment_homes` writes one copy
  // into the worktree the agent reads from and one into the durable store, and
  // where the second lives depends on how the bridge was started. Best effort —
  // a file left behind is four megabytes, not a broken stack.
  docker(
    `docker exec ${BRIDGE} sh -lc "find /worktrees /root/.build /home -name '${leaf}' -type f -delete 2>/dev/null; true"`,
    { quiet: true },
  );
};

// ── freeze the reader, and make sure it is always thawed again ───────────────
let frozen = [];
/** The browser processes this run launched, root first.
 *
 *  The root is found by PARENTAGE, not by name: Playwright spawns it as a direct
 *  child of this process, and that is the one fact about it that does not depend
 *  on the machine. Looking for the executable path instead does not work here —
 *  `/usr/bin/chromium` is resolved before `exec`, so every one of these processes
 *  reports `/usr/lib/chromium/chromium`, and the only row that DOES carry the path
 *  asked for is the shell that launched this script. (Nor is
 *  `browser.process()` available: this Playwright's `Browser` does not have it.)
 *
 *  Then the descendants, because Chromium spreads its work over a renderer, a GPU
 *  process and a network service, and the association is not read by the root.
 *  Crashpad handlers reparent away from this tree and are left alone — they read
 *  no channel.
 */
const launchedTree = () => {
  const parsed = execSync("ps -eo pid=,ppid=,args= -ww", { encoding: "utf8" })
    .split("\n")
    .map((row) => row.trim().match(/^(\d+)\s+(\d+)\s+(.*)$/))
    .filter(Boolean)
    .map(([, pid, ppid, args]) => ({ pid: Number(pid), ppid: Number(ppid), args }));
  const root = parsed.find((one) => one.ppid === process.pid && /chromium/i.test(one.args));
  if (!root) return [];
  const tree = [root.pid];
  for (let at = 0; at < tree.length; at++) {
    for (const one of parsed) if (one.ppid === tree[at] && !tree.includes(one.pid)) tree.push(one.pid);
  }
  return tree;
};
const signalAll = (pids, signal) => {
  for (const pid of pids) {
    try {
      process.kill(pid, signal);
    } catch {
      /* already gone */
    }
  }
};
/** Thawing is guaranteed on any exit. A run killed mid-freeze would otherwise
 *  leave a stopped Chromium holding the stack's session open for ever, and the
 *  next holder's failure would look like their own. */
const thaw = () => {
  if (!frozen.length) return;
  signalAll(frozen.reverse(), "SIGCONT");
  frozen = [];
};
for (const signal of ["exit", "SIGINT", "SIGTERM"]) process.on(signal, thaw);

const browser = await chromium.launch({ executablePath: CHROMIUM, headless: true, args: ["--no-sandbox"] });
try {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
  await page.fill('input[name="email"]', "qa@localhost");
  await Promise.all([
    page.waitForNavigation({ waitUntil: "load" }),
    page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
  ]);

  const route = `${APP}/app/#/project/${seed.projectId}/workspace/${workspace.workspaceId}`;
  await page.goto(route, { waitUntil: "load" });
  await page.waitForTimeout(12000);
  const sessions = () =>
    page.evaluate(() => (globalThis.buildConnectionDiagnostics?.() || []).filter((e) => e.event === "connected").length);
  const live = await sessions();
  record("the workspace stands up with the bridge answering", live > 0, `${live} connected session(s)`);

  // ── the fetch goes out, and the reader stops reading ──────────────────────
  //
  // Fired and not awaited: the point is to have it in flight. The reply is ~5.6 MB
  // of base64 against a 1 MiB send buffer, so the bridge's writer is still going
  // when the freeze lands, and a frozen process stops draining its sockets.
  const chip = page.locator("button.thread-attachment").first();
  const chipThere = await chip.count();
  record("the seeded file is on screen as something to fetch", chipThere > 0, `${chipThere} file chip(s)`);
  // A chip, deliberately, not an image: a picture is fetched the moment the
  // timeline renders, which would put the reply on the wire before the freeze
  // could land. A file is fetched when it is pressed, so the press and the freeze
  // are one beat apart. The press itself may be refused a download in headless
  // Chromium; the fetch it started is what matters.
  await chip.click({ timeout: 5000 }).catch(() => {});

  const tree = launchedTree();
  if (!tree.length) throw new Error("could not find this run's Chromium processes to freeze");
  console.log(`\nfreezing ${tree.length} browser process(es) for ${Math.round(FREEZE_MS / 1000)}s — reading nothing\n`);
  frozen = tree;
  signalAll(tree, "SIGSTOP");
  const frozenAt = Date.now();
  // Nothing may touch the page while it is stopped: every assertion in here is
  // read from the bridge's own log.
  await new Promise((resolve) => setTimeout(resolve, FREEZE_MS));
  const duringFreeze = bridgeLog();
  thaw();
  const thawedAfterMs = Date.now() - frozenAt;

  const stallLine = duringFreeze.split("\n").find((line) => line.includes("write_stalled"));
  record(
    `the bridge says its own frames are not leaving, inside ${Math.round(FREEZE_MS / 1000)}s`,
    Boolean(stallLine),
    stallLine ? stallLine.trim().slice(-160) : "no write_stalled line in the bridge log",
  );
  for (const field of ["channel=app", "buffered_bytes=", "stalled_ms=", "channel_age_ms="]) {
    record(`…and the line carries ${field.replace(/[=]$/, "")}`, Boolean(stallLine?.includes(field)), stallLine ? "" : "no line");
  }
  const stalledMs = Number(stallLine?.match(/stalled_ms=(\d+)/)?.[1] ?? NaN);
  record(
    `the stall is judged at the timeout it names, not at SCTP's ~${SCTP_GAVE_UP_AFTER_MS / 1000}s`,
    Number.isFinite(stalledMs) && stalledMs >= timing.stallSeconds * 1000 && stalledMs < SCTP_GAVE_UP_AFTER_MS,
    Number.isFinite(stalledMs) ? `stalled_ms=${stalledMs}, timeout ${timing.stallSeconds * 1000} ms` : "no stalled_ms",
  );
  const buffered = Number(stallLine?.match(/buffered_bytes=(\d+)/)?.[1] ?? NaN);
  record(
    "…with bytes actually queued behind it",
    Number.isFinite(buffered) && buffered > 0,
    Number.isFinite(buffered) ? `buffered_bytes=${buffered}, send-buffer limit ${timing.bufferedHigh}` : "no buffered_bytes",
  );
  // Scoped to the session that stalled, by the id on its own line. An unscoped
  // match passes on any session's channel ending — and the first run of this
  // harness did exactly that, reporting a close that belonged to a different
  // session entirely while the stalled one's was never checked.
  const stalledSession = stallLine?.match(/session="([^"]+)"/)?.[1] ?? "";
  const closedIt = stalledSession
    ? duringFreeze
        .split("\n")
        .filter((line) => line.includes(`session="${stalledSession}"`) && /channel=app (closed|reader_ended)/.test(line))
    : [];
  record(
    "the app channel of THAT session is closed rather than left retransmitting",
    closedIt.length > 0,
    closedIt.length ? closedIt.at(-1).trim().slice(-120) : `nothing closed for ${stalledSession || "an unknown session"}`,
  );

  // ── the client comes back ────────────────────────────────────────────────
  console.log(`\nthawed after ${Math.round(thawedAfterMs / 1000)}s — waiting for a new session\n`);
  let reconnected = null;
  for (let step = 0; step < 45 && reconnected === null; step++) {
    await page.waitForTimeout(2000);
    if ((await sessions().catch(() => 0)) > live) reconnected = Date.now() - frozenAt - FREEZE_MS;
  }
  record(
    "the client mints a new session once it is reading again",
    reconnected !== null,
    reconnected === null ? "never reconnected in 90s" : `connected again at +${Math.round(reconnected / 1000)}s after the thaw`,
  );
} finally {
  thaw();
  await browser.close().catch(() => {});
  removeSeededFile();
}

console.log("\n──────── summary ────────");
for (const { name, ok, detail } of results) console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
const passed = results.filter((one) => one.ok).length;
console.log(`\n${passed}/${results.length} passed`);
process.exit(passed === results.length ? 0 : 1);
