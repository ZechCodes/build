// The real SPA peer link and crypto; only its rendezvous is a fixture.
import * as transport from "/node_modules/@build/secure-transport/src/index.js";
import { openCarrier, peerFrames } from "/src/core/carrier.js";
import { openPeerLink } from "/src/core/peerLink.js";
import { createSessionRpc } from "/src/core/sessionRpc.js";
import { connectionDiagnosticHistory } from "/src/core/connectionDiagnostics.js";

const SESSION_ID = "lan-upgrade-fixture";
const DEVICE_ID = "dev-1";
const ICE_SERVERS = [{ urls: ["turn:198.18.0.1:3478?transport=udp"], username: "fixture", credential: "fixture-password" }];
const waitFor = async (predicate, deadline, description) => {
  while (!(await predicate())) {
    if (Date.now() >= deadline) throw new Error(`deadline: ${description}`);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
};

// Plaintext mock rendezvous on an isolated namespace's fixed private address.
// nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
const fixture = new WebSocket("ws://10.72.0.1:9000");
const firstMessage = () => new Promise((resolve, reject) => {
  const timeout = setTimeout(() => reject(new Error("fixture WebSocket message deadline")), 5000);
  fixture.addEventListener("message", (event) => { clearTimeout(timeout); resolve(JSON.parse(event.data)); }, { once: true });
});
const pinnedKey = (await firstMessage()).transport_public_key;
await window.fixtureReady();

export async function run() {
  const mode = await window.fixtureMode();
  console.log("minting fixture session");
  const init = await transport.createSessionInit({ sessionId: SESSION_ID, deviceId: DEVICE_ID, deviceTransportPublicKeyB64: pinnedKey });
  const acceptance = firstMessage();
  fixture.send(JSON.stringify({ type: "session_init", session_id: SESSION_ID, session_init: init.sessionInit }));
  await transport.openSessionAccept({ sessionKeyB64: init.sessionKeyB64, envelope: (await acceptance).envelope });
  console.log("session accepted");
  const rpc = createSessionRpc({ transport, sessionId: SESSION_ID, sessionKeyB64: init.sessionKeyB64, deviceId: DEVICE_ID });
  const signaling = openCarrier({ socket: fixture, sessionId: SESSION_ID });
  rpc.readFrom(signaling);
  let peer;
  let connections = 0;
  let restarts = 0;
  let hostCandidate;
  let hostGated = false;
  let releaseHostTrickle;
  let hostTrickled = false;
  const hostTrickleReady = new Promise((resolve) => { releaseHostTrickle = resolve; });
  const connectionStateListeners = new Set();
  function TrackedPeer(config) {
    connections += 1;
    peer = new RTCPeerConnection(config);
    const addListener = peer.addEventListener.bind(peer);
    const removeListener = peer.removeEventListener.bind(peer);
    peer.addEventListener = (type, listener, options) => {
      if (type === "connectionstatechange") connectionStateListeners.add(listener);
      return addListener(type, listener, options);
    };
    peer.removeEventListener = (type, listener, options) => {
      if (type === "connectionstatechange") connectionStateListeners.delete(listener);
      return removeListener(type, listener, options);
    };
    const createOffer = peer.createOffer.bind(peer);
    peer.createOffer = (options) => {
      if (options?.iceRestart) restarts += 1;
      return createOffer(options);
    };
    return peer;
  }
  const signal = async (method, params) => {
      console.log("signal", method);
      if (method === "rtc.ice") {
        console.log("candidate", params.candidate.candidate);
        const fields = params.candidate.candidate.split(/\s+/);
        if (fields[7] === "host" && fields[4].endsWith(".local")) {
          const gatheredHost = { name: fields[4], port: Number(fields[5]), gatheredAt: Date.now() };
          await window.fixtureHostPort(gatheredHost);
          if (!hostCandidate) {
            hostCandidate = gatheredHost;
            await window.fixtureHost(hostCandidate);
            hostGated = true;
            if (["unresolved", "late-unresolved"].includes(mode)) await hostTrickleReady;
          }
        }
      }
      const answer = await rpc.call(method, params, { carrier: signaling, timeoutMs: 10000 });
      if (method === "rtc.ice" && params.candidate.candidate.includes(".local")) hostTrickled = true;
      return answer;
  };
  const link = mode === "far-edge-pressure"
    ? await openPressureControl({ signal, onPush: (callback) => rpc.onPush(callback),
      TrackedPeer, hostGated: () => hostGated })
    : await openPeerLink({
    signal,
    fetchIceServers: async () => ICE_SERVERS,
    onPush: (callback) => rpc.onPush(callback),
    RTCPeerConnectionImpl: TrackedPeer,
    diagnosticId: SESSION_ID,
  });
  rpc.readFrom(link.app);
  rpc.rideOn(link.app);
  const stats = async () => {
    const reports = [...(await peer.getStats()).values()];
    const pairDetails = (pair) => {
      const local = reports.find((row) => row.id === pair.localCandidateId);
      const remote = reports.find((row) => row.id === pair.remoteCandidateId);
      return { id: pair.id, state: pair.state, nominated: pair.nominated, localType: local?.candidateType,
        remoteType: remote?.candidateType, bytesSent: pair.bytesSent, bytesReceived: pair.bytesReceived,
        localPort: local?.port, remotePort: remote?.port,
        requestsSent: pair.requestsSent, responsesReceived: pair.responsesReceived };
    };
    const selectedId = reports.find((row) => row.type === "transport")?.selectedCandidatePairId;
    return { selected: reports.filter((row) => row.id === selectedId).map(pairDetails)[0],
      hostSocketPort: reports.find((row) => row.type === "remote-candidate" && row.candidateType === "host")?.port,
      direct: reports.filter((row) => row.type === "candidate-pair").map(pairDetails)
        .filter((row) => row.localType !== "relay" && row.remoteType !== "relay"),
      restarts, connections, localUfrag: peer.localDescription.sdp.match(/a=ice-ufrag:(\S+)/)?.[1],
      remoteUfrag: peer.remoteDescription.sdp.match(/a=ice-ufrag:(\S+)/)?.[1] };
  };
  window.fixtureSnapshot = async () => ({ stats: await stats(), diagnostics: connectionDiagnosticHistory() });
  const appRpcPaths = [];
  const applicationCall = async (method, params) => {
    if (["early-unresolved", "far-edge-unresolved"].includes(mode)) {
      const state = await stats();
      if (state.selected?.state !== "succeeded" || !state.selected.nominated
        || state.selected.localType === "relay" || state.selected.remoteType === "relay"
        || link.transportPath() !== "direct") {
        throw new Error(`early application RPC must use direct: ${method} ${JSON.stringify(state)}`);
      }
      appRpcPaths.push({ at: Date.now(), method, selected: state.selected, path: link.transportPath() });
    }
    return rpc.call(method, params);
  };
  const pull = async () => {
    const board = await applicationCall("board.list");
    const { projects } = await applicationCall("project.list");
    if (!Array.isArray(projects) || projects.length !== 1) throw new Error(`fixture project missing: ${JSON.stringify(projects)}`);
    const tasks = await applicationCall("tasks.list", { project_id: projects[0].project_id });
    await applicationCall("ping");
    return { board, projectCount: projects.length, tasks, sessionId: rpc.sessionId };
  };
  if (["early-unresolved", "far-edge-unresolved"].includes(mode)) {
    // The genuine host trickle arrives immediately in this case. Record the
    // selected native pair before the first encrypted application RPC, and
    // again at every application call, without changing production timing.
    const before = await stats();
    if (!hostCandidate || before.restarts !== 0 || before.connections !== 1) {
      throw new Error("the early win must use its original real host candidate and peer");
    }
    await applicationCall("session.hello");
    const directPull = await pull();
    await waitFor(async () => {
      const current = await stats();
      return current.selected?.bytesSent > before.selected.bytesSent
        && current.selected?.bytesReceived > before.selected.bytesReceived;
    }, Date.now() + 1500, "the initially direct pair carries the complete encrypted pull");
    const after = await stats();
    if (after.restarts !== 0 || after.connections !== 1 || after.selected.id !== before.selected.id
      || connectionDiagnosticHistory().some((row) => row.phase === "mdns-resolved")) {
      throw new Error("the early direct win must keep one peer, zero restarts and unresolved mDNS");
    }
    const result = { mode, before, after, hostCandidate, directPull, appRpcPaths, diagnostics: connectionDiagnosticHistory() };
    await window.fixtureRelease(result);
    await rpc.call("rtc.close", {}, { carrier: signaling, timeoutMs: 10000 });
    link.close("early unresolved mDNS namespace fixture finished");
    fixture.close();
    return result;
  }
  await applicationCall("session.hello");
  const before = await stats();
  if (before.selected?.localType !== "relay" && before.selected?.remoteType !== "relay") throw new Error(`initial path must use TURN: ${JSON.stringify(before)}`);
  if (!hostCandidate) throw new Error("Chromium must gather a real UUID.local host candidate");
  if (connectionDiagnosticHistory().some((row) => row.phase === "mdns-resolved")) throw new Error("the browser name resolved before the fixture released its answer");
  const turnPull = await pull();
  await window.fixtureRelease({ before, turnPull, hostCandidate });
  if (mode === "unresolved") {
    // Preserve the production early probe. Delay only this genuine trickle
    // until a full encrypted TURN pull has completed, so this case proves
    // that one already-working session can upgrade rather than starting direct.
    releaseHostTrickle();
    await waitFor(() => hostTrickled, Date.now() + 10000, "the bridge acknowledges the genuine unresolved host trickle");
  }
  let exhausted;
  let exhaustedCheckWindow;
  if (mode === "late-unresolved") {
    const initialPairs = new Set(before.direct.map((pair) => pair.id));
    if (!initialPairs.size) throw new Error("the late fixture must first observe an active native host pair");
    await waitFor(() => window.fixtureInitialChecksExhausted(hostCandidate.port, before.selected.remotePort),
      Date.now() + 15000, "Chromium completes its measured unanswered host-check budget before the authentic trickle arrives");
    exhausted = await stats();
    exhaustedCheckWindow = await window.fixtureInitialCheckWindow(hostCandidate.port, before.selected.remotePort);
    await window.fixtureExhausted({ exhausted, hostCandidate,
      checkWindow: exhaustedCheckWindow });
    releaseHostTrickle();
    await waitFor(() => hostTrickled, Date.now() + 10000, "the bridge acknowledges the genuine late host trickle");
  }
  if (mode !== "delayed") {
    const result = await unresolvedUpgrade({ mode, before, turnPull, stats, pull, link, exhausted, exhaustedCheckWindow });
    await rpc.call("rtc.close", {}, { carrier: signaling, timeoutMs: 10000 });
    link.close("unresolved mDNS namespace fixture finished");
    fixture.close();
    return result;
  }
  const discovery = () => connectionDiagnosticHistory().find((row) => row.event === "candidate-diagnostics" && row.phase === "mdns-resolved");
  await waitFor(discovery, Date.now() + 4000, "bridge resolves the delayed Chromium name");
  // This assertion is deliberately before the SPA's existing 20s mDNS restart
  // fallback. Main resolves the name but sends zero host checks at this point.
  await waitFor(async () => (await window.fixtureChecks()) > 0, Date.now() + 6000,
    "bridge sends outbound late-host STUN checks after TURN nomination");
  const checkingBefore = await stats();
  const isPending = (pair) => ["waiting", "in-progress"].includes(pair.state);
  if (!checkingBefore.direct.some(isPending)) throw new Error(`direct pair must be pending during pull: ${JSON.stringify(checkingBefore)}`);
  const checkingPull = await pull();
  await waitFor(async () => {
    const current = await stats();
    return current.selected?.bytesSent > checkingBefore.selected.bytesSent
      && current.selected?.bytesReceived > checkingBefore.selected.bytesReceived;
  }, Date.now() + 1500, "selected TURN pair accounts for the full pull while host checks run");
  const checkingAfter = await stats();
  if (!checkingAfter.direct.some(isPending)) throw new Error("host checks completed before the gated TURN pull");
  if (checkingAfter.selected.id !== before.selected.id) throw new Error("the full pull must stay on the selected TURN pair while host checks run");
  await window.fixtureReleaseChecks();
  await waitFor(async () => (await stats()).direct.some((row) => row.state === "succeeded"), Date.now() + 6000,
    "browser sees a succeeded direct pair before any ICE restart");
  const viable = await stats();
  if (viable.restarts !== 0) throw new Error("the browser direct pair must succeed before the SPA restarts ICE");
  await window.fixtureViable({ viable, checkingBefore, checkingAfter, checkingPull, diagnostics: connectionDiagnosticHistory() });
  await waitFor(async () => {
    const state = await stats();
    return state.restarts === 1 && state.selected?.state === "succeeded" && state.selected?.nominated
      && state.selected.localType !== "relay" && state.selected.remoteType !== "relay"
      && state.localUfrag !== before.localUfrag && state.remoteUfrag !== before.remoteUfrag
      && state.selected.id !== viable.selected.id
      && !link.recovery.snapshot().recovering && link.transportPath() === "direct";
  }, Date.now() + 35000, "production directPairMonitor nominates direct after one ICE restart");
  const directBefore = await stats();
  const directPull = await pull();
  await waitFor(async () => {
    const current = await stats();
    return current.selected?.bytesSent > directBefore.selected.bytesSent
      && current.selected?.bytesReceived > directBefore.selected.bytesReceived;
  }, Date.now() + 1500, "selected direct pair accounts for the full pull after the ICE restart");
  const after = await stats();
  if (after.selected.id !== directBefore.selected.id) throw new Error("the final full pull must stay on its nominated direct pair");
  if (after.connections !== 1) throw new Error("the optional upgrade must reuse its one RTCPeerConnection");
  const diagnostics = connectionDiagnosticHistory();
  const attempt = diagnostics.find((row) => row.event === "direct-pair" && row.state === "trying");
  if (!["direct-pair-succeeded", "mdns-resolved"].includes(attempt?.reason)) throw new Error(`unexpected restart evidence: ${JSON.stringify(attempt)}`);
  const result = { before, turnPull, viable, checkingBefore, checkingAfter, checkingPull, after, directPull, diagnostics };
  if (diagnostics.filter((row) => row.event === "direct-pair" && row.state === "trying").length !== 1) {
    throw new Error("the encrypted session gets exactly one optional direct upgrade");
  }
  // This final phase isolates the bridge's native failure cleanup. Disable
  // browser recovery only after the complete production upgrade proof, so
  // its recovery offer cannot replace the generation before the bridge's
  // default 5+25-second ICE Failed timeout. Native ICE and its socket stay on.
  for (const listener of connectionStateListeners) peer.removeEventListener("connectionstatechange", listener);
  await window.fixtureDisconnectHost();
  await waitFor(() => window.fixtureBridgeFailed(), Date.now() + 32000, "bridge reaches its actual ICE Failed state");
  result.failed = await stats();
  if (result.failed.restarts !== after.restarts || result.failed.connections !== 1) {
    throw new Error("failure proof must retain the same connection and its actual restart count");
  }
  // The rendezvous remains the session's encrypted signaling carrier; it can
  // close a peer whose application channels ICE has just stopped carrying.
  await rpc.call("rtc.close", {}, { carrier: signaling, timeoutMs: 10000 });
  link.close("namespace fixture finished");
  fixture.close();
  return result;
}

window.runLanUpgrade = run;

async function unresolvedUpgrade({ mode, before, turnPull, stats, pull, link, exhausted, exhaustedCheckWindow }) {
  const isDirect = (state) => state.selected?.state === "succeeded" && state.selected.nominated
    && state.selected.localType !== "relay" && state.selected.remoteType !== "relay"
    && link.transportPath() === "direct" && !link.recovery.snapshot().recovering;
  const unresolved = () => !connectionDiagnosticHistory().some((row) => row.phase === "mdns-resolved");
  if (mode === "far-edge-pressure") {
    await waitFor(() => window.fixtureSweepExpired(), Date.now() + 30000,
      "the no-direct control reaches the production 25-second sweep expiry");
    const after = await stats();
    if (after.selected?.localType !== "relay" && after.selected?.remoteType !== "relay") {
      throw new Error("the pressure control must retain its genuine TURN pair");
    }
    if (after.restarts !== 0 || after.connections !== 1 || !unresolved()) {
      throw new Error("the isolated pressure control has one native peer and no restart monitor");
    }
    const turnAfter = await pull();
    const droppedHostChecks = await window.fixtureDroppedHostChecks();
    if (droppedHostChecks === 0) throw new Error("the namespace must prove it suppressed actual native host checks");
    return { mode, control: "native peer without SPA optional monitor; namespace host-check DROP",
      before, after, turnPull, turnAfter, droppedHostChecks, diagnostics: connectionDiagnosticHistory() };
  }
  if (mode === "large-subnet") {
    await waitFor(() => window.fixtureSweepSkipped(), Date.now() + 6000, "oversize on-link subnet is skipped");
    // Include the real SPA monitor's first 20-second sample: an unresolved
    // host and a skipped sweep must not trigger an optional ICE restart.
    await waitFor(() => Date.now() - beforeAt() >= 22000, Date.now() + 23000, "TURN remains selected across the monitor period");
    const after = await stats();
    const selectedTurn = after.selected?.localType === "relay" || after.selected?.remoteType === "relay";
    if (!selectedTurn || isDirect(after) || after.restarts !== 0 || after.connections !== 1 || !unresolved()) {
      throw new Error(`oversize unresolved subnet must stay on TURN: ${JSON.stringify(after)}`);
    }
    return { mode, before, turnPull, after, turnAfter: await pull(), diagnostics: connectionDiagnosticHistory() };
  }
  let primed;
  if (mode === "late-unresolved") {
    await waitFor(() => window.fixtureSweepPrimed(), Date.now() + 6000, "the late indication reaches the real phone from the advertised host socket");
    primed = await stats();
    if (isDirect(primed) || primed.restarts !== 0) throw new Error("the exhausted host pair must stay on TURN after the real late phone indication");
    await window.fixtureLatePrimed({ primed, exhausted });
  }
  await waitFor(async () => isDirect(await stats()), Date.now() + 32000,
    "the same unresolved-mDNS encrypted session upgrades via authenticated peer-reflexive checks");
  const directBefore = await stats();
  const directPull = await pull();
  await waitFor(async () => {
    const current = await stats();
    return current.selected?.bytesSent > directBefore.selected.bytesSent
      && current.selected?.bytesReceived > directBefore.selected.bytesReceived;
  }, Date.now() + 1500, "direct prflx pair carries the full encrypted pull");
  const after = await stats();
  if (!unresolved()) throw new Error("the permanently silenced mDNS responder must never resolve");
  if (after.connections !== 1 || after.restarts > 1) throw new Error("sweep upgrade must reuse its connection and bounded optional restart");
  if (mode === "late-unresolved" && (after.restarts !== 1 || after.localUfrag === before.localUfrag || after.remoteUfrag === before.remoteUfrag)) {
    throw new Error("the late sweep needs exactly one real ICE restart with fresh credentials");
  }
  return { mode, before, turnPull, exhausted, exhaustedCheckWindow, primed, after, directPull, diagnostics: connectionDiagnosticHistory() };
}

function beforeAt() {
  return connectionDiagnosticHistory().find((row) => row.event === "connected")?.at ?? Date.now();
}

// An isolated socket-pressure control, not a production-monitor test. It uses
// real native SDP, candidates, channels and encrypted SessionRPC, while the
// namespace drops host checks so the bridge can measure its full sweep window.
async function openPressureControl({ signal, onPush, TrackedPeer, hostGated }) {
  const deadline = Date.now() + 15000;
  const peer = new TrackedPeer({ iceServers: ICE_SERVERS });
  const channels = ["app", "term"].map((label, id) => peer.createDataChannel(label, { negotiated: true, id, ordered: true }));
  const frames = peerFrames();
  const carriers = channels.map((channel) => openCarrier({ channel, frames }));
  const queued = [];
  const errors = [];
  const add = (candidate) => peer.addIceCandidate(candidate).catch((error) => errors.push(String(error)));
  const unsubscribe = onPush((push) => {
    if (push.type !== "rtc.ice") return;
    if (peer.remoteDescription) void add(push.candidate);
    else queued.push(push.candidate);
  });
  peer.addEventListener("icecandidate", (event) => {
    if (event.candidate) void signal("rtc.ice", { candidate: event.candidate.toJSON() })
      .catch((error) => errors.push(String(error)));
  });
  const offer = await peer.createOffer();
  await peer.setLocalDescription(offer);
  const answer = await signal("rtc.offer", { sdp: offer.sdp, ice_servers: ICE_SERVERS });
  await waitFor(hostGated, deadline, "namespace host-check filter is installed before applying the real answer");
  await peer.setRemoteDescription({ type: "answer", sdp: answer.sdp });
  for (const candidate of queued) await add(candidate);
  await waitFor(() => {
    if (errors.length) throw new Error(`native pressure control signaling: ${errors.join("; ")}`);
    return peer.connectionState === "connected" && channels.every((channel) => channel.readyState === "open");
  }, deadline, "native pressure-control TURN channels open");
  frames.connected = true;
  return { app: carriers[0], close() { unsubscribe(); carriers.forEach((carrier) => carrier.close()); peer.close(); } };
}
