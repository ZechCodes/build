// End-to-end probe of the 1.1 wire surface, over a real E2EE session.
//
// Everything the "Bridge Wire Protocol Spec" Part 1 + Part 2 added, exercised
// against a live app + relay + bridge (deploy/compose.real.yml): the greeting's
// version and capabilities, `bridge.stats`' client census and priority queues,
// `changes.subscribe` in both cadences, the `changes` push a real filesystem
// write produces, the envelope's `"priority": "background"` and the structured
// error codes.
//
// It rides the same wire the SPA does: the relay socket is a rendezvous that
// mints this device's session and carries the peer negotiation, and it is shut
// before the first check runs (strict P2P transport spec, rules 2 and 4). The
// `changes` pushes below arrive over the `app` DataChannel.
//
// It talks to the stack from the host (the relay's and the api's published
// ports) so it can make the bridge's repository move the way a human would —
// with a write from outside the daemon:
//
//   docker compose -f deploy/compose.real.yml up -d --build
//   docker compose -f deploy/compose.real.yml --profile qa run --rm qa   # pair
//   cd web && node wire-check.mjs
//
// Env: API_URL, RELAY_URL, QA_EMAIL, BRIDGE_EXEC (how to run a shell in the
// bridge container), BRIDGE_FILE (the file inside its sample repo to append to).

import { execFile } from "node:child_process";
import { promisify } from "node:util";
import * as transport from "@build/secure-transport";
import { openDeviceLink, openRendezvous } from "./client.mjs";
import { loginWithDummy } from "./skrift-auth.mjs";

const execFileAsync = promisify(execFile);
const apiUrl = process.env.API_URL || "http://127.0.0.1:8090";
const relayUrl = process.env.RELAY_URL || "ws://127.0.0.1:18090";
const preferDeviceId = process.env.PREFER_DEVICE_ID || null;
const bridgeExec = (process.env.BRIDGE_EXEC ||
  "docker compose -f ../deploy/compose.real.yml exec -T bridge").split(/\s+/);
const bridgeFile = process.env.BRIDGE_FILE || "/repo/README.md";
const API_RANGE = ">=1.0.0 <2.0.0";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const show = (value) => JSON.stringify(value);

// ------------------------------------------------------------- reporting ---

const results = [];
function check(id, name, ok, evidence) {
  results.push({ id, name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${id}. ${name}`);
  if (evidence !== undefined) console.log(`      ${evidence}`);
}

// -------------------------------------------------------------- the wire ---

// A live E2EE session over this device's `app` channel: `call` for RPC,
// `pushes` for every server-initiated frame, `frames` for every decrypted
// payload (so an `ok:false` reply can be read as the bridge wrote it, error
// code and all). The relay socket that found the device is closed by the time
// this returns.
async function session(login) {
  const rendezvous = await openRendezvous({ relayUrl, mintGatewayToken: login.mintGatewayToken });
  if (!rendezvous.authenticated) throw new Error(`relay refused the token: ${show(rendezvous.ack)}`);

  const pushes = [];
  const frames = [];
  const link = await openDeviceLink({
    rendezvous,
    transport,
    apiUrl,
    cookie: login.cookie,
    preferDeviceId,
    onPush: (push) => pushes.push({ at: Date.now(), push }),
    onFrame: (frame) => frames.push(frame),
  });
  if (!rendezvous.isClosed()) throw new Error("the relay socket is still open under a live peer connection");
  const { call } = link.session;

  // `call` rejects on `ok:false` with the message alone; the raw reply is in
  // `frames`, which is what the error-code checks assert against.
  const attempt = async (method, params, envelopeFields) => {
    try {
      await call(method, params, envelopeFields);
    } catch {
      /* the frame is the assertion, not the throw */
    }
    return frames.findLast((frame) => frame && frame.ok !== undefined) || null;
  };

  return { call, attempt, pushes, frames, close: () => link.close() };
}

// Every `changes` push seen since `from`, newest last.
const changesSince = (pushes, from) =>
  pushes.filter(({ at, push }) => at >= from && push.type === "changes").map(({ push }) => push);

async function waitFor(predicate, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const found = predicate();
    if (found) return found;
    if (Date.now() >= deadline) return null;
    await sleep(50);
  }
}

// Append a line to a file in the bridge's sample repository, from outside the
// daemon — the change the watcher exists to notice.
async function writeIntoSampleRepo(marker) {
  const [command, ...args] = bridgeExec;
  await execFileAsync(command, [...args, "sh", "-c", `echo ${marker} >> ${bridgeFile}`]);
}

// ---------------------------------------------------------------- checks ---

async function greetingAndProbe(s) {
  // The one greeting: subscriptions is the only mode a bridge serves.
  const hello = await s.call("session.hello", {
    client: { name: "wire-check", version: "0", api_range: API_RANGE },
    changes: "subscriptions",
  });
  if (hello.changes?.mode !== "subscriptions") {
    throw new Error(`session.hello did not answer in subscriptions mode: ${show(hello.changes)}`);
  }
  check(
    "a",
    'session.hello reports 1.1.0, push_events and the changes capability',
    hello.api_version === "1.1.0" && hello.push_events === true && hello.changes?.subscriptions === true,
    show(hello),
  );

  const ping = await s.call("ping");
  check("b", "ping carries api_version", ping.api_version === "1.1.0", show(ping));
}

async function stats(s) {
  const result = await s.call("bridge.stats");
  const counted = result.clients?.[API_RANGE];
  check(
    "c",
    "bridge.stats counts this client under its api_range and reports both queues",
    counted >= 1 && !!result.queues?.foreground && !!result.queues?.background,
    `clients=${show(result.clients)} queues=${show(result.queues)}`,
  );
}

async function subscribe(s, entityId) {
  const realtime = await s.call("changes.subscribe", {
    subscription_id: "wc-realtime",
    scope: { kind: "entity", id: entityId },
    kinds: ["state", "thread", "git", "files"],
    mode: "realtime",
    priority: "foreground",
  });
  const batch = await s.call("changes.subscribe", {
    subscription_id: "wc-batch",
    scope: { kind: "entity", id: entityId },
    kinds: ["git"],
    mode: { batch_ms: 5000 },
    priority: "background",
  });
  const live = realtime.watch === "live" && batch.watch === "live";
  check(
    "d",
    "changes.subscribe answers both cadences with a subscription_id and a watch",
    realtime.subscription_id === "wc-realtime" &&
      batch.subscription_id === "wc-batch" &&
      !!realtime.watch &&
      !!batch.watch,
    `realtime=${show(realtime)} batch=${show(batch)}${live ? "" : "  (watch is not \"live\": a worktree in scope could not get a filesystem watcher, so git/files come from the TTL refresh)"}`,
  );
}

async function pushes(s, entityId) {
  const from = Date.now();
  const marker = `wire-check-${Date.now()}`;
  await writeIntoSampleRepo(marker);
  const relative = bridgeFile.replace(/^\/repo\/?/, "");

  const realtime = await waitFor(() => {
    const frame = changesSince(s.pushes, from).find((push) => push.subscription_id === "wc-realtime");
    return frame?.items?.some((item) => item.entity_id === entityId) ? frame : null;
  }, 2000);
  const item = realtime?.items?.find((entry) => entry.entity_id === entityId);
  check(
    "e1",
    "a write reaches the realtime subscription within 2s with files.paths and git.status_key",
    item?.files?.paths?.includes(relative) && typeof item?.git?.status_key === "string",
    realtime ? show(realtime) : `no changes frame for wc-realtime within 2s (saw ${show(changesSince(s.pushes, from))})`,
  );

  const batch = await waitFor(() => {
    const frame = changesSince(s.pushes, from).find((push) => push.subscription_id === "wc-batch");
    return frame?.items?.some((entry) => entry.entity_id === entityId) ? frame : null;
  }, 7000);
  const batchItem = batch?.items?.find((entry) => entry.entity_id === entityId);
  const onlyGit = batchItem && Object.keys(batchItem).sort().join(",") === "entity_id,git";
  check(
    "e2",
    "the batch subscription's item arrives within ~6s carrying only git",
    onlyGit,
    batch ? `${show(batch)}  (+${Date.now() - from}ms)` : "no changes frame for wc-batch within 7s",
  );
}

async function backgroundPriority(s, projectId) {
  const status = await s.call("git.status", { project_id: projectId }, { priority: "background" });
  check(
    "f",
    'git.status with "priority":"background" in the envelope answers normally',
    Array.isArray(status.files) || typeof status.status_key === "string",
    show(status).slice(0, 300),
  );
}

async function errors(s) {
  const unknown = await s.attempt("does.not.exist", {});
  check(
    "g1",
    'an unknown method answers ok:false with a string error and error_code "unknown_method"',
    unknown?.ok === false && typeof unknown.error === "string" && unknown.error_code === "unknown_method",
    show(unknown),
  );

  const malformed = await s.attempt("git.status", { project_id: 7, if_status_key: [] });
  check(
    "g2",
    'git.status with malformed params answers error_code "invalid_params"',
    malformed?.ok === false && malformed.error_code === "invalid_params",
    show(malformed),
  );
}

async function unsubscribed(s) {
  await s.call("changes.unsubscribe", { subscription_id: "wc-realtime" });
  await s.call("changes.unsubscribe", { subscription_id: "wc-batch" });
  const from = Date.now();
  await writeIntoSampleRepo(`wire-check-after-unsubscribe-${Date.now()}`);
  await sleep(3000);
  const seen = changesSince(s.pushes, from);
  check("h", "no changes frame arrives within 3s of unsubscribing", seen.length === 0, `saw ${show(seen)}`);
}

// ------------------------------------------------------------------ main ---

async function main() {
  const login = await loginWithDummy(apiUrl, {
    email: process.env.QA_EMAIL || "qa@localhost",
  });

  const s = await session(login);
  await greetingAndProbe(s);
  await stats(s);

  const { projects } = await s.call("project.list");
  const project = projects?.find((entry) => entry.is_git) || projects?.[0];
  if (!project) throw new Error("no project on the bridge — is BRIDGE_REPO adopted?");
  console.log(`\n  sample repo entity: ${project.project_id} (${project.path})\n`);

  await subscribe(s, project.project_id);
  await pushes(s, project.project_id);
  await backgroundPriority(s, project.project_id);
  await errors(s);
  await unsubscribed(s);
  s.close();

  const failed = results.filter((result) => !result.ok);
  console.log(`\n${results.length - failed.length}/${results.length} wire checks passed`);
  if (failed.length) {
    console.error("WIRE-CHECK FAIL:", failed.map((result) => result.id).join(", "));
    process.exit(1);
  }
  console.log("WIRE-CHECK PASS: the 1.1 wire surface answers end-to-end");
  process.exit(0);
}

main().catch((error) => {
  console.error("WIRE-CHECK ERROR:", error.stack || error.message);
  process.exit(1);
});
