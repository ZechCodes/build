import { openCarrier, peerFrames } from "./carrier.js";
import { classifyTransportPath, TURN } from "./transportPath.js";
import { createRelayHold } from "./iceCandidates.js";
import { recordConnectionDiagnostic } from "./connectionDiagnostics.js";

const CHANNELS = [["app", 0], ["term", 1]];
const OPEN_TIMEOUT_MS = 15000;
const blockedBy = (reason, message) => Object.assign(new Error(message), { blockedReason: reason });
const safeState = (state) => (["new", "connecting", "connected", "disconnected", "failed", "closed"].includes(state) ? state : "unknown");
const safeIceState = (state) => (["new", "checking", "connected", "completed", "disconnected", "failed", "closed"].includes(state) ? state : "unknown");

function createRecoveryStatus() {
  const listeners = new Set();
  let state = { epoch: 0, recovering: false };
  const transition = (recovering) => {
    state = { epoch: state.epoch + 1, recovering };
    for (const listener of [...listeners]) listener(state);
  };
  return {
    snapshot: () => state,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    begin: () => transition(true),
    end: () => transition(false),
    clear: () => listeners.clear(),
  };
}

export async function openPeerLink({ signal, fetchIceServers, onPush, onConnected = () => {}, onFailed = () => {},
  RTCPeerConnectionImpl = globalThis.RTCPeerConnection, openTimeoutMs = OPEN_TIMEOUT_MS, diagnosticId = "peer" }) {
  const diagnostic = (event, detail = {}) => recordConnectionDiagnostic(diagnosticId, event, detail);
  const iceServers = await withinDeadline(openTimeoutMs, () => fetchIceServers());
  const peer = new RTCPeerConnectionImpl({ iceServers });
  const channels = CHANNELS.map(([label, id]) => peer.createDataChannel(label, { negotiated: true, id, ordered: true }));
  // One clock for both channels: they are one path, and a frame on either is
  // proof it is up. What reads it is the terminal session's liveness probe,
  // which must not judge a quiet channel beside a busy one as a dead peer.
  const frames = peerFrames();
  const carriers = channels.map((channel) => openCarrier({ channel, frames }));
  const recovery = createRecoveryStatus();
  // How this connection ended up carrying — straight to the machine, or
  // through a TURN relay. Sampled when it lands and again after every ICE
  // restart, because a restart is where a path that was direct becomes a
  // relayed one. Null until the first sample answers.
  let transportPath = null;
  const sampleTransportPath = async () => {
    try {
      const path = classifyTransportPath(await peer.getStats?.());
      if (torn || !path) return;
      transportPath = path;
      diagnostic("carrying", { path });
    } catch {
      /* a peer that cannot be asked says nothing, and the reader is told
         nothing rather than told wrong */
    }
  };
  let torn = false;
  let tornBecause = "closed by the client";
  let cancelWait = () => {};
  let stopWatching = () => {};
  const observed = [];
  const observe = (target, type, listener) => {
    target.addEventListener(type, listener);
    observed.push(() => target.removeEventListener(type, listener));
  };
  const ensureActive = () => {
    if (torn) throw blockedBy("failed", "the peer connection closed");
  };
  // Relay candidates are held behind the direct ones at BOTH doors (issue #31).
  // One hold per direction, because the two check lists are different lists: ours
  // is built from the candidates the bridge sends us, and the bridge's from the
  // ones we send it. Holding only our own outgoing candidates would tidy the
  // bridge's race and leave the browser's — and the browser is the offerer, so
  // the browser is the controlling agent, and nomination is the browser's.
  const holdInbound = createRelayHold({
    deliver: (candidate) => {
      if (torn) return;
      peer.addIceCandidate(candidate).catch(() => diagnostic("candidate-failed", { direction: "remote" }));
    },
  });
  const holdOutbound = createRelayHold({
    deliver: (gathered) => {
      if (torn) return;
      // Held as the RTCIceCandidate — that is the shape whose `type` the hold
      // reads — and serialised only on the way out, because `toJSON` does not
      // carry it.
      const candidate = gathered.toJSON ? gathered.toJSON() : gathered;
      signal("rtc.ice", { candidate }).catch(() => diagnostic("candidate-failed", { direction: "local" }));
    },
  });
  const unsubscribe = onPush((push) => {
    if (push.type !== "rtc.ice" || torn) return;
    holdInbound.offer(push.candidate);
  });
  const outgoingCandidate = (event) => {
    if (!event.candidate || torn) return;
    holdOutbound.offer(event.candidate);
  };
  peer.addEventListener("icecandidate", outgoingCandidate);
  /** What ICE says about this path, for everything riding it to read. The
   *  browser runs consent checks of its own (RFC 7675) and takes the state off
   *  `connected` when they stop being answered, so this is a better answer to
   *  "is the path there" than any silence an application can measure. */
  const readIceState = () => {
    frames.connected = ["connected", "completed"].includes(peer.iceConnectionState)
      || peer.connectionState === "connected";
  };
  observe(peer, "connectionstatechange", () => {
    readIceState();
    diagnostic("state", { state: safeState(peer.connectionState) });
  });
  observe(peer, "iceconnectionstatechange", () => {
    readIceState();
    diagnostic("ice-state", { state: safeIceState(peer.iceConnectionState) });
  });
  for (const channel of channels) {
    observe(channel, "close", () => diagnostic("channel", { channel: channel.label, state: "closed" }));
    observe(channel, "error", () => diagnostic("channel", { channel: channel.label, state: "error" }));
  }
  /** Take the connection down, saying why.
   *
   *  The reason is not decoration: a peer that closed with nothing said is a
   *  reconnect nobody can account for afterwards, and that is exactly the
   *  state a browser re-minting its session every six seconds leaves its
   *  reader in. Every caller names one. */
  const tearDown = (reason = "closed by the client") => {
    if (torn) return;
    torn = true;
    tornBecause = reason;
    cancelWait();
    stopWatching();
    recovery.clear();
    holdInbound.close();
    holdOutbound.close();
    unsubscribe();
    for (const stopObserving of observed.splice(0)) stopObserving();
    peer.removeEventListener("icecandidate", outgoingCandidate);
    frames.connected = false;
    for (const carrier of carriers) carrier.close();
    peer.close();
    diagnostic("closed", { reason: tornBecause });
    signal("rtc.close", {}).catch(() => {});
  };

  try {
    diagnostic("negotiating", { phase: "initial" });
    await withinDeadline(openTimeoutMs, async (remaining) => {
      await offer(peer, signal, iceServers, {}, ensureActive);
      await usable(peer, channels, remaining(), diagnostic, (cancel) => (cancelWait = cancel), ensureActive, true);
    }, (cancel) => (cancelWait = cancel));
    readIceState();
    // The race this hold was protecting is decided: a relay candidate arriving
    // now cannot displace what is already nominated, so there is nothing left to
    // buy by delaying it — and a session that later needs TURN to survive an ICE
    // restart wants every candidate it can get.
    holdInbound.stopHolding();
    holdOutbound.stopHolding();
    diagnostic("connected", { phase: "initial" });
    await sampleTransportPath();
  } catch (error) {
    tearDown(`the connection never opened: ${error?.blockedReason || "failed"}`);
    throw error;
  }

  stopWatching = watchForFailure(peer, diagnostic, async () => {
    recovery.begin();
    try {
      await withinDeadline(openTimeoutMs, async (remaining) => {
        await onFailed();
        if (torn) return;
        diagnostic("restarting");
        const freshServers = await fetchIceServers();
        ensureActive();
        peer.setConfiguration?.({ iceServers: freshServers });
        await offer(peer, signal, freshServers, { iceRestart: true }, ensureActive);
        await usable(peer, channels, remaining(), diagnostic, (cancel) => (cancelWait = cancel), ensureActive);
      }, (cancel) => (cancelWait = cancel));
      if (torn) return;
      readIceState();
      diagnostic("connected", { phase: "restart" });
      await sampleTransportPath();
      recovery.end();
      await onConnected();
    } catch (error) {
      diagnostic("restart-failed", { reason: error?.blockedReason === "timeout" ? "timeout" : "failed" });
      tearDown("the ICE restart did not land");
    }
  });
  const [app, term] = carriers;
  return { app, term, recovery, transportPath: () => transportPath, close: tearDown };
}

async function offer(peer, signal, iceServers, options, ensureActive) {
  const local = await peer.createOffer(options);
  ensureActive();
  await peer.setLocalDescription(local);
  ensureActive();
  const answer = await signal("rtc.offer", { sdp: local.sdp, ice_servers: iceServers });
  ensureActive();
  await peer.setRemoteDescription({ type: "answer", sdp: answer.sdp });
  ensureActive();
}

function usable(peer, channels, timeoutMs, diagnostic, registerCancel, ensureActive, failFast = false) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const settle = (answer) => {
      if (settled) return;
      settled = true;
      clearTimeout(deadline);
      peer.removeEventListener("connectionstatechange", changed);
      for (const channel of channels) { channel.removeEventListener("open", changed); channel.removeEventListener("close", changed); }
      answer();
    };
    const changed = () => {
      try { ensureActive(); } catch (error) { settle(() => reject(error)); return; }
      diagnostic("state", { state: safeState(peer.connectionState) });
      if (peer.connectionState === "closed" || (failFast && peer.connectionState === "failed")) settle(() => reject(blockedBy("failed", `the peer connection ${peer.connectionState} before it became usable`)));
      else if (peer.connectionState === "connected" && channels.every(({ readyState }) => readyState === "open")) settle(resolve);
    };
    const deadline = setTimeout(() => settle(() => reject(blockedBy("timeout", "the peer connection did not become usable"))), timeoutMs);
    registerCancel(() => settle(() => reject(blockedBy("failed", "the peer connection closed"))));
    peer.addEventListener("connectionstatechange", changed);
    for (const channel of channels) { channel.addEventListener("open", changed); channel.addEventListener("close", changed); }
    changed();
  });
}

function watchForFailure(peer, diagnostic, onFailed) {
  let restarting = false;
  const changed = async () => {
    diagnostic("state", { state: safeState(peer.connectionState) });
    if (!["failed", "disconnected"].includes(peer.connectionState) || restarting) return;
    restarting = true;
    await onFailed();
    restarting = false;
  };
  peer.addEventListener("connectionstatechange", changed);
  return () => peer.removeEventListener("connectionstatechange", changed);
}

async function withinDeadline(timeoutMs, work, registerCancel = () => {}) {
  const started = Date.now();
  let timer;
  const remaining = () => Math.max(1, timeoutMs - (Date.now() - started));
  const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(blockedBy("timeout", "the peer connection did not open before its negotiation timed out")), timeoutMs); });
  const cancelled = new Promise((_, reject) => registerCancel(() => reject(blockedBy("failed", "the peer connection closed"))));
  try { return await Promise.race([work(remaining), timeout, cancelled]); }
  finally { clearTimeout(timer); }
}
