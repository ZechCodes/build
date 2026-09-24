// #128 on a stack: does a phone-shaped session survive a bridge under load?
//
// The maintainer's phone reaches their bridge through TURN and drops its
// session when the host is busy — agents running, the tracker being read. This
// holds ONE session the way the phone does (every candidate relayed, a ping
// every two seconds with a three-second deadline, which is the SPA's liveness
// probe) for SOAK_MS, while the harness itself makes the load, and says whether
// the session lived: every ping's round trip, every ICE and connection state,
// every push, every channel close. Exit 0 only with no drop, no timeout and no
// ping over RTT_CEILING_MS; 1 when any of those happened; 3 when it was clean
// but the path was not relay/relay, so it said nothing about the phone.
//
// It runs INSIDE the qa container, which has node-datachannel and the
// secure-transport binding and reaches app, relay and coturn by name:
//
//   docker compose -p liveness128 -f deploy/compose.real.yml -f deploy/compose.liveness.yml \
//     --profile qa run --rm --no-deps -T -e SOAK_MS=180000 -e LOAD_TERM_THREADS=24 \
//     -e HAMMER_ISSUES=1 qa node liveness-soak.mjs </dev/null
//
// (deploy/compose.liveness.yml says how the stack comes up and is paired.)
// The qa image COPIES web/, so rebuild it after editing this file:
// `… --profile qa build qa`.
//
// # coturn, on the compose network
//
// Never --net=host: this host's firewall drops container→host TURN, so the
// bridge would allocate nothing and the pair would quietly be direct (#55).
//
//   docker run -d --name liveness128-turn --network liveness128_default coturn/coturn:latest \
//     -n --listening-port=3478 --fingerprint --lt-cred-mech --user=build:soak \
//     --realm=build.test --no-tls --no-cli --log-file=stdout
//   docker inspect liveness128-turn      # NetworkSettings.Networks.liveness128_default.IPAddress
//   docker rm -f liveness128-turn        # teardown
//
// # Forcing the relayed path: three levers, all here
//
//   1. this peer gets the coturn list and `iceTransportPolicy: "relay"`;
//   2. the same list rides `rtc.offer` as `ice_servers`, so the bridge allocates
//      through the same coturn (the SPA forwards its list the same way);
//   3. the bridge's host/srflx candidates are dropped on the way in, trickled or
//      in the answer. Lever 1 alone is NOT enough with node-datachannel: the
//      policy only filters which local candidates libdatachannel signals, and
//      libjuice still runs checks from its host socket, so with the bridge's host
//      candidate in the list the pair came out host/host (peer-reflexive on our
//      side). With only the bridge's relay address to talk to, and coturn holding
//      no permission for either host address, nothing but relay/relay can form.
//
// The nominated pair is read back from `selectedCandidatePair()` (libdatachannel)
// and printed; the bridge's own `rtc: … carrying over relay/relay` line agrees.
//
// # Knobs
//
//   SOAK_MS=600000  PING_EVERY_MS=2000  PING_DEADLINE_MS=3000  RTT_CEILING_MS=500
//   TURN_HOST=liveness128-turn  coturn's name or IP, resolved here so both ends get
//                          an address literal;  TURN_PORT TURN_USER TURN_PASSWORD
//   RELAY_BOTH_ENDS=1  ICE_TRANSPORT_POLICY=relay   levers 3 and 1, for experiments
//   LOAD_TERM_THREADS=N    a second session (`term` channel) opens a terminal in the
//                          first workspace (created if none) and starts N busy
//                          loops in it: load spawned the way a terminal agent's is.
//   LOAD_AGENT_THREADS=N   dispatches a HEADLESS agent (`branch.dispatch`, provider
//                          claude_adk) on the first project; the `claude` the bridge
//                          finds on its PATH must be the fake from the #128 proof
//                          (it spins BRIDGE_LOAD_THREADS busy loops as its tool
//                          children), so this is load spawned the way a real
//                          headless agent's is, in that agent's scope.
//   HAMMER_ISSUES=1        seed SEED_ISSUES (150) issues with a comment each in the
//                          first project, then issues.list back to back from a third
//                          session (foreground priority, the `app` channel) for the
//                          whole soak: the read that holds the app lock.
//   PREFER_DEVICE_ID       pin one device when the account has several
//
// Host load is the operator's, started before the run and killed after:
//
//   for i in $(seq N); do nice -n 19 sh -c 'while :; do :; done' & done
//   kill $(jobs -p)
//
// The bridge's own squeeze is in deploy/compose.liveness.yml; `docker update
// --cpus 0.5 liveness128-bridge-1` changes the quota live, keeping the pairing.

import { lookup } from "node:dns/promises";
import { monitorEventLoopDelay, performance } from "node:perf_hooks";
import { RTCPeerConnection } from "node-datachannel/polyfill";
import * as transport from "@build/secure-transport";
import { openRendezvous, pinnedDevice } from "./client.mjs";
import { createReassembler, openCarriedSession, splitEnvelope } from "./peer.mjs";
import { loginWithDummy } from "./skrift-auth.mjs";

const num = (name, fallback) => Number(process.env[name] || fallback);
const API_URL = process.env.API_URL || "http://app:8080";
const RELAY_URL = process.env.RELAY_URL || "ws://relay:8799"; // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
const SOAK_MS = num("SOAK_MS", 600000);
const PING_EVERY_MS = num("PING_EVERY_MS", 2000);
const PING_DEADLINE_MS = num("PING_DEADLINE_MS", 3000);
const RTT_CEILING_MS = num("RTT_CEILING_MS", 500);
const TURN = { host: process.env.TURN_HOST || "liveness128-turn", port: num("TURN_PORT", 3478), user: process.env.TURN_USER || "build", password: process.env.TURN_PASSWORD || "soak" };
const RELAY_BOTH_ENDS = process.env.RELAY_BOTH_ENDS !== "0";
const POLICY = process.env.ICE_TRANSPORT_POLICY || "relay";
const TERM_THREADS = num("LOAD_TERM_THREADS", 0);
const AGENT_THREADS = num("LOAD_AGENT_THREADS", 0);
const HAMMER = process.env.HAMMER_ISSUES === "1";
const SEED_ISSUES = num("SEED_ISSUES", 150);

const started = performance.now();
const stamp = () => `${((performance.now() - started) / 1000).toFixed(1).padStart(7)}s`;
const log = (...parts) => console.log(stamp(), ...parts);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const encode = (text) => Buffer.from(text, "utf8").toString("base64");
const candidateType = (line) => / typ (\w+)/.exec(line || "")?.[1] ?? "?";
const pct = (values, p) => [...values].sort((a, b) => a - b)[Math.min(values.length - 1, Math.floor((p / 100) * values.length))] ?? NaN;
const maxOf = (values) => values.reduce((most, value) => Math.max(most, value), -Infinity);
const ms = (value) => (Number.isFinite(value) ? `${Math.round(value)} ms` : "n/a");

const run = { sent: 0, rtts: [], timeouts: 0, refusals: 0, iceDisconnected: 0, drops: [], pushes: [], otherPushes: {}, states: [] };
const hammer = { calls: 0, errors: 0, latencies: [] };
let dead = null; // why the link can carry no more pings, once it cannot
let stopping = false;
const drop = (why) => (run.drops.push(`${stamp()} ${why}`), log(`DROP ${why}`));
const loopDelay = monitorEventLoopDelay({ resolution: 20 });
loopDelay.enable();

// ── sessions, minted on the rendezvous (client.mjs's `mint`, which it keeps private)
async function mint(rendezvous, device) {
  const sessionId = "sess-" + Math.random().toString(36).slice(2, 10);
  const { sessionKeyB64, sessionInit } = await transport.createSessionInit({ sessionId, deviceId: device.deviceId, deviceTransportPublicKeyB64: device.transportPublicKeyB64 });
  const accepted = new Promise((resolve, reject) => {
    const timer = setTimeout(() => (stop(), reject(new Error(`device did not accept ${sessionId}`))), 15000);
    const stop = rendezvous.onMessage((message) => {
      if (message.type !== "session_accept" || message.session_id !== sessionId) return;
      clearTimeout(timer);
      stop();
      resolve(message);
    });
  });
  rendezvous.send({ type: "session_init", session_id: sessionId, route_to: `device:${device.deviceId}`, session_init: sessionInit });
  await transport.openSessionAccept({ sessionKeyB64, envelope: (await accepted).envelope });
  return { sessionId, sessionKeyB64, deviceId: device.deviceId };
}

const relayCarrier = (rendezvous, sessionId) => ({
  send: (envelope) => rendezvous.send({ type: "e2ee_envelope", session_id: sessionId, envelope }),
  onEnvelope: (fn) => rendezvous.onMessage((m) => m.type === "e2ee_envelope" && (!m.session_id || m.session_id === sessionId) && fn(m.envelope)),
  close: () => {},
});

/** peer.mjs's channel carrier, plus the one thing this run is about: a channel
 *  that closes under us is a drop. */
function channelCarrier(channel) {
  const listeners = new Set();
  const reassembler = createReassembler();
  channel.addEventListener("message", (event) => {
    const text = typeof event.data === "string" ? event.data : Buffer.from(event.data).toString("utf8");
    let whole;
    try {
      whole = reassembler.accept(text);
    } catch (error) {
      drop(`${channel.label} channel: ${error.message}`);
      return channel.close();
    }
    if (whole !== null) for (const listener of [...listeners]) listener(JSON.parse(whole));
  });
  channel.addEventListener("close", () => {
    if (stopping) return;
    drop(`${channel.label} channel closed`);
    if (channel.label === "app") dead ??= "the app channel closed";
  });
  channel.addEventListener("error", (event) => log(`${channel.label} channel error: ${event?.error?.message || event?.message || "?"}`));
  return {
    send(envelope) {
      if (channel.readyState !== "open") throw new Error(`the ${channel.label} channel closed`);
      for (const part of splitEnvelope(JSON.stringify(envelope))) channel.send(part);
    },
    onEnvelope: (fn) => (listeners.add(fn), () => listeners.delete(fn)),
    close: () => channel.close(),
  };
}

/** One session's view of a channel two sessions share: it is only handed the
 *  envelopes stamped with its id, so a big answer for the other session is not
 *  a decrypt this one attempts and fails. */
const forSession = (carrier, sessionId) => ({
  ...carrier,
  onEnvelope: (fn) => carrier.onEnvelope((envelope) => (!envelope.session_id || envelope.session_id === sessionId) && fn(envelope)),
});

// ── the relayed peer connection ─────────────────────────────────────────────
const isRelay = (candidate) => candidateType(candidate?.candidate) === "relay";
const relayOnlySdp = (sdp) =>
  sdp.split(/\r?\n/).filter((line) => !line.startsWith("a=candidate:") || / typ relay( |$)/.test(line)).join("\r\n");
const placed = (candidate) => (candidate?.sdpMid ? candidate : { ...candidate, sdpMid: undefined });

function watch(peer) {
  const note = (kind, state) => {
    run.states.push(`${stamp()} ${kind}=${state}`);
    log(`${kind} → ${state}`);
  };
  peer.addEventListener("iceconnectionstatechange", () => {
    note("ice", peer.iceConnectionState);
    if (peer.iceConnectionState === "disconnected") run.iceDisconnected += 1;
  });
  peer.addEventListener("connectionstatechange", () => {
    note("peer", peer.connectionState);
    if (!stopping && ["failed", "closed"].includes(peer.connectionState)) {
      drop(`peer connection ${peer.connectionState}`);
      dead ??= `the peer connection ${peer.connectionState}`;
    }
  });
  peer.addEventListener("icegatheringstatechange", () => note("gathering", peer.iceGatheringState));
}

async function openRelayedLink({ signaling, iceServers }) {
  const peer = new RTCPeerConnection({ iceServers, iceTransportPolicy: POLICY });
  watch(peer);
  const channels = [["app", 0], ["term", 1]].map(([label, id]) => peer.createDataChannel(label, { negotiated: true, id, ordered: true }));
  const seen = { local: [], remote: [], refused: [] };
  signaling.onPush((push) => {
    if (push.type !== "rtc.ice") return;
    const type = candidateType(push.candidate?.candidate);
    if (RELAY_BOTH_ENDS && !isRelay(push.candidate)) return void seen.refused.push(type);
    seen.remote.push(type);
    peer.addIceCandidate(placed(push.candidate)).catch(() => {});
  });
  let offered = false;
  const waiting = [];
  const trickle = (candidate) => signaling.call("rtc.ice", { candidate }).catch(() => {});
  peer.addEventListener("icecandidate", (event) => {
    if (!event.candidate) return;
    const candidate = event.candidate.toJSON ? event.candidate.toJSON() : event.candidate;
    seen.local.push(candidateType(candidate.candidate));
    offered ? trickle(candidate) : waiting.push(candidate);
  });
  const offer = await peer.createOffer();
  await peer.setLocalDescription(offer);
  const answer = await signaling.call("rtc.offer", { sdp: offer.sdp, ice_servers: iceServers });
  offered = true;
  for (const candidate of waiting.splice(0)) trickle(candidate);
  const inAnswer = (answer.sdp.match(/^a=candidate:.*$/gm) || []).map(candidateType);
  await peer.setRemoteDescription({ type: "answer", sdp: RELAY_BOTH_ENDS ? relayOnlySdp(answer.sdp) : answer.sdp });
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("the relayed channels did not open in 30 s")), 30000);
    let left = channels.length;
    for (const channel of channels) channel.addEventListener("open", () => --left === 0 && (clearTimeout(timer), resolve()));
  });
  log(`channels open; local candidates [${seen.local}] bridge's in answer [${inAnswer}] trickled kept [${seen.remote}] dropped [${seen.refused}]`);
  const [app, term] = channels.map(channelCarrier);
  return { peer, app, term };
}

/** What ICE nominated, as libdatachannel reports it. */
function pairNow(peer) {
  const pair = peer.selectedCandidatePair?.();
  if (!pair) return { relayed: false, text: "none reported" };
  const end = (one) => `${one.type} ${one.address}:${one.port}`;
  return { relayed: /relay/i.test(pair.local.type) && /relay/i.test(pair.remote.type), text: `${pair.local.type}/${pair.remote.type} (local ${end(pair.local)} → remote ${end(pair.remote)})` };
}

// ── load: busy loops in a terminal, and the tracker read ─────────────────────
async function startBusyLoops(app, term) {
  const [project] = (await app.call("project.list")).projects || [];
  if (!project) throw new Error("the bridge has no project to open a terminal in");
  let [workspace] = (await app.call("workspace.list", { project_id: project.project_id })).workspaces || [];
  workspace ??= await app.call("workspace.create", { project_id: project.project_id, name: `liveness-${Date.now().toString(36)}`, isolation: "worktree" });
  const { term_id } = await term.call("term.create", { workspace_id: workspace.workspace_id, cols: 100, rows: 30 });
  await term.call("term.attach", { workspace_id: workspace.workspace_id, term_id, cols: 100, rows: 30 });
  // A subshell, so the loops share ONE foreground job: Ctrl-C reaches its trap
  // (background children of a non-interactive shell ignore SIGINT themselves),
  // and the hangup term.close causes reaches all of them. Left as bare `&` jobs
  // of the interactive shell they get process groups of their own and outlive
  // the terminal, spinning in the bridge's container forever.
  const line = `(trap 'kill -KILL 0' INT HUP TERM; for i in $(seq ${TERM_THREADS}); do (while :; do :; done) & done; wait)`;
  await term.call("term.input", { term_id, data: encode(`${line}\r`) });
  log(`${TERM_THREADS} busy loops started in terminal ${term_id} (workspace ${workspace.workspace_id})`);
  return async () => {
    await term.call("term.input", { term_id, data: encode("\x03") }).catch((error) => log(`Ctrl-C refused: ${error.message}`));
    await term.call("term.close", { term_id }).then(() => log(`terminal ${term_id} closed`), (error) => log(`term.close refused: ${error.message}`));
  };
}

/** A headless agent, dispatched the way the app does it; the fake `claude` on
 * the bridge's PATH does the spinning. */
async function startAgentLoad(session, project) {
  const branch = `liveness-load-${Date.now().toString(36)}`;
  const dispatched = await session.call("branch.dispatch", {
    project_id: project.project_id, instruction: `liveness load: spin ${AGENT_THREADS} busy loops`, branch,
    provider: "claude_adk", model: "claude-fable-5-1",
  });
  const runId = dispatched.run_id || dispatched.run?.run_id || dispatched.agent?.run_id || null;
  log(`headless agent dispatched on ${branch}: ${JSON.stringify(dispatched).slice(0, 200)}`);
  return async () => {
    if (!runId) return log("no run id to abandon; the bridge's stop ends the agent's scope");
    await session.call("run.abandon", { run_id: runId }).then(() => log(`run ${runId} abandoned`), (error) => log(`run.abandon refused: ${error.message}`));
  };
}

async function seedIssues(session, projectId) {
  const have = ((await session.call("issues.list", { project_id: projectId })).issues || []).length;
  const body = "Seeded by liveness-soak.mjs. ".repeat(12);
  for (let n = have; n < SEED_ISSUES; n++) {
    const { issue } = await session.call("issues.create", { project_id: projectId, title: `liveness seed ${n}`, body, labels: ["liveness", `batch-${n % 7}`] });
    await session.call("issues.comment", { issue_id: issue.id, body: `comment on ${n}: ${body}` });
  }
  log(`issues: ${have} were there, ${Math.max(0, SEED_ISSUES - have)} seeded with a comment each`);
}

async function hammerIssues(session, projectId) {
  while (!stopping && !dead) {
    const sent = performance.now();
    try {
      await session.call("issues.list", { project_id: projectId }, { priority: "foreground" });
      hammer.latencies.push(performance.now() - sent);
    } catch (error) {
      hammer.errors += 1;
      if (/channel closed/.test(error.message)) return;
      await sleep(250);
    }
    hammer.calls += 1;
  }
}

// ── the soak ─────────────────────────────────────────────────────────────────
function progress(link, lastMinute) {
  const hammerNote = HAMMER ? `  issues.list n=${hammer.calls} p50=${ms(pct(hammer.latencies, 50))} max=${ms(maxOf(hammer.latencies))}` : "";
  log(
    `progress: pings=${run.sent} last-minute p50=${ms(pct(lastMinute, 50))} max=${ms(maxOf(lastMinute))}` +
      `  timeouts=${run.timeouts} drops=${run.drops.length} ice=${link.peer.iceConnectionState} pair=${pairNow(link.peer).text}` +
      `  loop-delay max=${ms(loopDelay.max / 1e6)}${hammerNote}`,
  );
}

async function soak(link, session) {
  const until = Date.now() + SOAK_MS;
  let minute = [];
  let nextProgress = Date.now() + 60000;
  const TIMEOUT = Symbol("timeout");
  while (Date.now() < until && !dead && !stopping) {
    const tick = Date.now();
    const sent = performance.now();
    run.sent += 1;
    const answer = session.call("ping").then(() => performance.now() - sent, (error) => error);
    const first = await Promise.race([answer, sleep(PING_DEADLINE_MS).then(() => TIMEOUT)]);
    if (first === TIMEOUT) {
      run.timeouts += 1;
      log(`ping TIMEOUT: no pong in ${PING_DEADLINE_MS} ms`);
      answer.then((late) => {
        if (typeof late !== "number") return;
        run.rtts.push(late);
        log(`  …its pong came after ${Math.round(late)} ms`);
      });
    } else if (first instanceof Error) {
      if (/channel closed/.test(first.message)) dead ??= first.message;
      else run.refusals += 1;
      drop(`ping failed: ${first.message}`);
    } else {
      run.rtts.push(first);
      minute.push(first);
      if (first >= RTT_CEILING_MS) log(`slow ping: ${Math.round(first)} ms`);
    }
    if (Date.now() >= nextProgress) {
      progress(link, minute);
      minute = [];
      nextProgress += 60000;
    }
    await sleep(Math.max(0, tick + PING_EVERY_MS - Date.now()));
  }
}

function summary(link) {
  const pair = pairNow(link.peer);
  const rtts = run.rtts;
  const max = maxOf(rtts);
  const over = rtts.filter((rtt) => rtt >= RTT_CEILING_MS).length;
  console.log("\n──────── liveness summary ────────");
  console.log(`load         LOAD_TERM_THREADS=${TERM_THREADS} LOAD_AGENT_THREADS=${AGENT_THREADS} HAMMER_ISSUES=${HAMMER ? 1 : 0} SOAK_MS=${SOAK_MS} RELAY_BOTH_ENDS=${RELAY_BOTH_ENDS ? 1 : 0} ICE_TRANSPORT_POLICY=${POLICY}`);
  console.log(`path         ${pair.text}${pair.relayed ? "" : "   !! NOT relay/relay"}`);
  console.log(`pings        samples=${run.sent} answered=${rtts.length} p50=${ms(pct(rtts, 50))} p95=${ms(pct(rtts, 95))} max=${ms(max)}`);
  console.log(`             over ${RTT_CEILING_MS} ms=${over}  timeouts(>${PING_DEADLINE_MS} ms)=${run.timeouts}  never answered=${run.sent - rtts.length - run.refusals}  refused=${run.refusals}`);
  console.log(`ice          disconnected=${run.iceDisconnected}  states: ${run.states.map((line) => line.trim()).join(" | ")}`);
  console.log(`drops        ${run.drops.length}${run.drops.length ? "\n  " + run.drops.join("\n  ") : ""}${dead ? `\n  link dead: ${dead}` : ""}`);
  console.log(`pushes       soak session ${run.pushes.length} [${[...new Set(run.pushes)].join(", ")}]  other sessions ${JSON.stringify(run.otherPushes)}`);
  if (HAMMER) console.log(`issues.list  calls=${hammer.calls} p50=${ms(pct(hammer.latencies, 50))} p95=${ms(pct(hammer.latencies, 95))} max=${ms(maxOf(hammer.latencies))} errors=${hammer.errors}`);
  console.log(`harness      event-loop delay p99=${ms(loopDelay.percentile(99) / 1e6)} max=${ms(loopDelay.max / 1e6)} (a stall here is the harness's, not the bridge's)`);
  const failed = run.drops.length > 0 || run.timeouts > 0 || max >= RTT_CEILING_MS || dead;
  const verdict = failed ? "FAIL" : pair.relayed ? "PASS" : "INCONCLUSIVE (clean, but not relay/relay)";
  console.log(`RESULT       ${verdict}`);
  return failed ? 1 : pair.relayed ? 0 : 3;
}

async function main() {
  const { address } = await lookup(TURN.host, { family: 4 });
  const iceServers = [{ urls: [`turn:${address}:${TURN.port}?transport=udp`], username: TURN.user, credential: TURN.password }];
  log(`coturn ${TURN.host} = ${iceServers[0].urls[0]}; soak ${SOAK_MS} ms, ping every ${PING_EVERY_MS} ms, deadline ${PING_DEADLINE_MS} ms`);

  const { cookie, mintGatewayToken } = await loginWithDummy(API_URL, { email: process.env.QA_EMAIL || "qa@localhost" });
  const rendezvous = await openRendezvous({ relayUrl: RELAY_URL, mintGatewayToken });
  if (!rendezvous.authenticated) throw new Error(`relay refused the gateway token: ${JSON.stringify(rendezvous.ack)}`);
  if (transport.ready) await transport.ready();
  const device = await pinnedDevice({ apiUrl: API_URL, cookie, preferDeviceId: process.env.PREFER_DEVICE_ID || null });
  const soakMint = await mint(rendezvous, device);
  const termMint = TERM_THREADS > 0 ? await mint(rendezvous, device) : null;
  const hammerMint = HAMMER ? await mint(rendezvous, device) : null;

  const signaling = openCarriedSession({ carrier: relayCarrier(rendezvous, soakMint.sessionId), transport, ...soakMint });
  const link = await openRelayedLink({ signaling, iceServers });
  const other = (push) => (run.otherPushes[push.type] = (run.otherPushes[push.type] || 0) + 1);
  // Long call deadlines: the soak judges its own pings, and a pong that comes late
  // is a measurement, not an error.
  const session = openCarriedSession({ carrier: forSession(link.app, soakMint.sessionId), transport, ...soakMint, timeoutMs: 120000,
    onPush: (push) => { run.pushes.push(push.type); log(`push ${JSON.stringify(push).slice(0, 200)}`); } });
  const term = termMint && openCarriedSession({ carrier: link.term, transport, ...termMint, timeoutMs: 60000, onPush: other });
  const reader = hammerMint && openCarriedSession({ carrier: forSession(link.app, hammerMint.sessionId), transport, ...hammerMint, timeoutMs: 120000, onPush: other });
  for (const one of [session, term, reader].filter(Boolean)) await one.call("ping"); // each rides its channel before the socket goes
  await rendezvous.close();
  log(`device ${device.name} (${device.deviceId}); rendezvous closed; pair ${pairNow(link.peer).text}`);
  await session.call("session.hello", { client: { name: "liveness-soak", version: "0", api_range: ">=1.0.0 <2.0.0" } });

  const [project] = (await session.call("project.list")).projects || [];
  if (HAMMER) await seedIssues(reader, project.project_id);
  const stopLoops = term ? await startBusyLoops(session, term) : null;
  const stopAgent = AGENT_THREADS > 0 ? await startAgentLoad(session, project) : null;
  for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => ((stopping = true), log(`${signal}: ending the soak`)));

  loopDelay.reset();
  const reading = HAMMER ? hammerIssues(reader, project.project_id) : null;
  await soak(link, session);
  stopping = true;
  await reading;
  if (stopLoops) await stopLoops();
  if (stopAgent) await stopAgent();
  const code = summary(link);
  link.peer.close();
  process.exit(code);
}

main().catch((error) => {
  console.error(`liveness-soak: ${error.stack || error.message}`);
  process.exit(2);
});
