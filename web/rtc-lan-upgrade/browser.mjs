// The real SPA peer link and crypto; only its rendezvous is a fixture.
import * as transport from "/node_modules/@build/secure-transport/src/index.js";
import { openCarrier } from "/src/core/carrier.js";
import { openPeerLink } from "/src/core/peerLink.js";
import { createSessionRpc } from "/src/core/sessionRpc.js";
import { connectionDiagnosticHistory } from "/src/core/connectionDiagnostics.js";

const SESSION_ID = "lan-upgrade-fixture";
const DEVICE_ID = "dev-1";
const ICE_SERVERS = [{ urls: ["turn:10.72.0.2:3478?transport=udp"], username: "fixture", credential: "fixture-password" }];
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
  function TrackedPeer(config) {
    connections += 1;
    peer = new RTCPeerConnection(config);
    const createOffer = peer.createOffer.bind(peer);
    peer.createOffer = (options) => {
      if (options?.iceRestart) restarts += 1;
      return createOffer(options);
    };
    return peer;
  }
  const link = await openPeerLink({
    signal: async (method, params) => {
      console.log("signal", method);
      if (method === "rtc.ice") {
        console.log("candidate", params.candidate.candidate);
        const fields = params.candidate.candidate.split(/\s+/);
        if (fields[7] === "host" && fields[4].endsWith(".local") && !hostCandidate) {
          hostCandidate = { name: fields[4], port: Number(fields[5]) };
          await window.fixtureHost(hostCandidate);
        }
      }
      return rpc.call(method, params, { carrier: signaling, timeoutMs: 10000 });
    },
    fetchIceServers: async () => ICE_SERVERS,
    onPush: (callback) => rpc.onPush(callback),
    RTCPeerConnectionImpl: TrackedPeer,
    diagnosticId: SESSION_ID,
  });
  rpc.readFrom(link.app);
  rpc.rideOn(link.app);
  await rpc.call("session.hello");
  const stats = async () => {
    const reports = [...(await peer.getStats()).values()];
    const pairDetails = (pair) => {
      const local = reports.find((row) => row.id === pair.localCandidateId);
      const remote = reports.find((row) => row.id === pair.remoteCandidateId);
      return { id: pair.id, state: pair.state, nominated: pair.nominated, localType: local?.candidateType,
        remoteType: remote?.candidateType, bytesSent: pair.bytesSent, bytesReceived: pair.bytesReceived,
        requestsSent: pair.requestsSent, responsesReceived: pair.responsesReceived };
    };
    const selectedId = reports.find((row) => row.type === "transport")?.selectedCandidatePairId;
    return { selected: reports.filter((row) => row.id === selectedId).map(pairDetails)[0],
      direct: reports.filter((row) => row.type === "candidate-pair").map(pairDetails)
        .filter((row) => row.localType !== "relay" && row.remoteType !== "relay"),
      restarts, connections, localUfrag: peer.localDescription.sdp.match(/a=ice-ufrag:(\S+)/)?.[1],
      remoteUfrag: peer.remoteDescription.sdp.match(/a=ice-ufrag:(\S+)/)?.[1] };
  };
  const pull = async () => {
    const board = await rpc.call("board.list");
    const { projects } = await rpc.call("project.list");
    if (!Array.isArray(projects) || projects.length !== 1) throw new Error(`fixture project missing: ${JSON.stringify(projects)}`);
    const tasks = await rpc.call("tasks.list", { project_id: projects[0].project_id });
    await rpc.call("ping");
    return { board, projectCount: projects.length, tasks, sessionId: rpc.sessionId };
  };
  const before = await stats();
  if (before.selected?.localType !== "relay" && before.selected?.remoteType !== "relay") throw new Error(`initial path must use TURN: ${JSON.stringify(before)}`);
  if (!hostCandidate) throw new Error("Chromium must gather a real UUID.local host candidate");
  window.fixtureSnapshot = async () => ({ stats: await stats(), diagnostics: connectionDiagnosticHistory() });
  if (connectionDiagnosticHistory().some((row) => row.phase === "mdns-resolved")) throw new Error("the browser name resolved before the fixture released its answer");
  const turnPull = await pull();
  await window.fixtureRelease({ before, turnPull, hostCandidate });
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
  link.close("namespace fixture finished");
  fixture.close();
  return result;
}

window.runLanUpgrade = run;
