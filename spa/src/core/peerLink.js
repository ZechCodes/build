import { openCarrier, peerFrames } from "./carrier.js";
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
  let torn = false;
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
  const unsubscribe = onPush((push) => {
    if (push.type !== "rtc.ice" || torn) return;
    peer.addIceCandidate(push.candidate).catch(() => diagnostic("candidate-failed", { direction: "remote" }));
  });
  const outgoingCandidate = (event) => {
    if (!event.candidate || torn) return;
    const candidate = event.candidate.toJSON ? event.candidate.toJSON() : event.candidate;
    signal("rtc.ice", { candidate }).catch(() => diagnostic("candidate-failed", { direction: "local" }));
  };
  peer.addEventListener("icecandidate", outgoingCandidate);
  observe(peer, "connectionstatechange", () => diagnostic("state", { state: safeState(peer.connectionState) }));
  observe(peer, "iceconnectionstatechange", () => diagnostic("ice-state", { state: safeIceState(peer.iceConnectionState) }));
  for (const channel of channels) {
    observe(channel, "close", () => diagnostic("channel", { channel: channel.label, state: "closed" }));
    observe(channel, "error", () => diagnostic("channel", { channel: channel.label, state: "error" }));
  }
  const tearDown = () => {
    if (torn) return;
    torn = true;
    cancelWait();
    stopWatching();
    recovery.clear();
    unsubscribe();
    for (const stopObserving of observed.splice(0)) stopObserving();
    peer.removeEventListener("icecandidate", outgoingCandidate);
    for (const carrier of carriers) carrier.close();
    peer.close();
    diagnostic("closed");
    signal("rtc.close", {}).catch(() => {});
  };

  try {
    diagnostic("negotiating", { phase: "initial" });
    await withinDeadline(openTimeoutMs, async (remaining) => {
      await offer(peer, signal, iceServers, {}, ensureActive);
      await usable(peer, channels, remaining(), diagnostic, (cancel) => (cancelWait = cancel), ensureActive, true);
    }, (cancel) => (cancelWait = cancel));
    diagnostic("connected", { phase: "initial" });
  } catch (error) {
    tearDown();
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
      diagnostic("connected", { phase: "restart" });
      recovery.end();
      await onConnected();
    } catch (error) {
      diagnostic("restart-failed", { reason: error?.blockedReason === "timeout" ? "timeout" : "failed" });
      tearDown();
    }
  });
  const [app, term] = carriers;
  return { app, term, recovery, close: tearDown };
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
