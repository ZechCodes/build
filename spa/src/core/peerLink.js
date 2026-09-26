import { openCarrier, peerFrames } from "./carrier.js";
import { classifyTransportPath, TURN } from "./transportPath.js";
import { createRelayHold, directPairWorthTrying } from "./iceCandidates.js";
import { recordConnectionDiagnostic } from "./connectionDiagnostics.js";

const CHANNELS = [["app", 0], ["term", 1]];
const OPEN_TIMEOUT_MS = 15000;

/** How long a relayed session runs before it is worth one attempt at a direct
 *  pair (issue #31).
 *
 *  Late on purpose. An ICE restart on a live connection re-gathers and re-checks
 *  everything, and doing it early would put the cost on every session that
 *  happened to land on TURN during a slow first connect — including one that is
 *  still settling. Twenty seconds is past anything the connect sequence itself
 *  does, so what is left is a session that is simply on the wrong path. */
const RELAY_UPGRADE_AFTER_MS = 20000;
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

/** What a failed restart is recorded as: the two causes worth telling apart
 *  in a report, and everything else. */
const restartFailure = (error) =>
  (["timeout", "not-carried"].includes(error?.blockedReason) ? error.blockedReason : "failed");

/**
 * `confirmCarried` answers whether the session is carried over this link once a
 * restart says `connected` (#123) — the session's own ping, since only an
 * answer that crossed the channels proves they reach the process holding it.
 */
export async function openPeerLink({ signal, fetchIceServers, onPush, onConnected = () => {}, onFailed = () => {},
  confirmCarried,
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
  /** Told when the path this connection is carrying on CHANGES — which happens
   *  when it first lands, and again if a restart or the direct-pair attempt
   *  re-nominates. A subscription rather than a callback parameter, so nothing
   *  above has to be handed down into the link to hear about it. */
  const pathListeners = new Set();
  /** Told when a failed path has been put right and carries again (#123). The
   *  restart may have landed on a NEW bridge process — one that answered the
   *  offer and carries, and holds nothing of this session — so whoever owns
   *  the session greets it again. */
  const restoredListeners = new Set();
  /** This connection's stats, or null when the peer cannot be asked. Two readings
   *  are taken off them — which path is carrying, and whether a direct pair is
   *  worth trying — and neither may throw at its caller. */
  const peerStats = async () => {
    try {
      return await peer.getStats?.();
    } catch {
      return null;
    }
  };
  const sampleTransportPath = async () => {
    try {
      const path = classifyTransportPath(await peerStats());
      if (torn || !path) return;
      const moved = path !== transportPath;
      transportPath = path;
      // Recorded on every landing, changed or not: "the restart came back on
      // relay again" is worth having in the timeline. Announced only on a
      // change, because that is what a surface has to redraw for.
      diagnostic("carrying", { path });
      if (moved) for (const listener of [...pathListeners]) listener(path);
    } catch {
      /* a peer that cannot be asked says nothing, and the reader is told
         nothing rather than told wrong */
    }
  };
  let torn = false;
  let tornBecause = "closed by the client";
  let cancelWait = () => {};
  let stopWatching = () => {};
  /** Whether a negotiation is running. One at a time: two `createOffer` calls
   *  over one peer would race each other's local description, and the failure
   *  watcher and the direct-pair attempt can both want one. */
  let renegotiating = false;
  /** Whether a failed path is being put right in place: the restart the
   *  failure watcher runs, and the carry check after it. Not the optional
   *  direct-pair attempt, whose path works throughout. What the ring reads to
   *  say the machine is being reconnected to (#123), announced through
   *  `recovery`'s transitions. */
  let restoring = false;
  /** A caller that cannot ask its session is taken at ICE's word, as before. */
  const carried = async () => (confirmCarried ? confirmCarried() : true);
  /** The timer that will ask whether this relayed session could be direct, and
   *  whether it has already been asked. Once per connection: a session that has
   *  had its second run at the race does not get a third. */
  let upgradeTimer = null;
  let upgradeAsked = false;
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
    clearTimeout(upgradeTimer);
    recovery.clear();
    pathListeners.clear();
    restoredListeners.clear();
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

  /**
   * This session landed on TURN. Is there a direct pair it could have had?
   *
   * Asked once, late, and only on evidence (issue #31). ICE nominates the first
   * pair that connects and never re-nominates, so a session that lost the race on
   * a network where a direct pair also works is billed for TURN egress for its
   * whole life — and the only lever left after nomination is an ICE restart,
   * which re-runs the race.
   *
   * The bar for asking is that a non-relay pair has ALREADY SUCCEEDED in this
   * connection's own check list (`directPairWorthTrying`). That is what makes the
   * disturbance defensible: the browser has proved the direct pair carries, so
   * this is a second run at a race the relay merely answered first, not a gamble
   * on a path that might work. A symmetric NAT — which is what TURN is for — has
   * no such pair, is not asked, and is left alone.
   *
   * A failure costs nothing but the attempt: the session stays on relay. The one
   * exception is a restart that left the path itself broken, which is torn down,
   * because sitting on a dead peer is worse than reconnecting.
   */
  /** Whether there is anything here worth an ICE restart, on evidence. A
   *  connection that is already direct, already asked, or already renegotiating
   *  is not asked; nor is one whose check list holds no direct pair that worked,
   *  and that answer is final — the list is settled by now and will not become
   *  yes by waiting. */
  const directPairIsWorthTrying = async () => {
    if (upgradeAsked || torn || renegotiating || transportPath !== TURN) return false;
    const stats = await peerStats();
    if (torn) return false;
    if (directPairWorthTrying(stats)) return true;
    diagnostic("direct-pair", { state: "none-to-try" });
    return false;
  };

  /** What the second run at the race landed on. */
  const reportDirectPair = () =>
    diagnostic("direct-pair", {
      state: transportPath === TURN ? "stayed-relayed" : "renominated",
      path: transportPath,
    });

  /** The attempt did not land. The session stays on relay, which is the whole
   *  point of it being optional — unless the restart left the path itself
   *  broken, because sitting on a dead peer is worse than reconnecting. */
  const directPairFailed = (error) => {
    diagnostic("direct-pair", { state: "failed", reason: error?.blockedReason === "timeout" ? "timeout" : "failed" });
    if (!torn && ["failed", "disconnected"].includes(peer.connectionState)) {
      tearDown("the direct-pair attempt left the path failed");
    }
  };

  /** Let go of what the attempt took, whichever way it went. `onConnected` is
   *  the rendezvous lease `renegotiate` acquired: a lease held after a failed
   *  attempt would keep a relay socket open for the life of the session, which
   *  rule 1 does not allow. */
  const directPairFinished = async () => {
    renegotiating = false;
    if (torn) return;
    recovery.end();
    await onConnected();
  };

  const attemptDirectPair = async () => {
    if (!(await directPairIsWorthTrying())) return;
    upgradeAsked = true;
    renegotiating = true;
    diagnostic("direct-pair", { state: "trying" });
    recovery.begin();
    try {
      await renegotiate("direct-pair");
      readIceState();
      await sampleTransportPath();
      reportDirectPair();
    } catch (error) {
      directPairFailed(error);
    } finally {
      await directPairFinished();
    }
  };

  /** Arm the one attempt, if this session landed anywhere worth asking about. */
  const armDirectPairAttempt = () => {
    if (upgradeAsked || torn || transportPath !== TURN) return;
    clearTimeout(upgradeTimer);
    upgradeTimer = setTimeout(() => void attemptDirectPair(), RELAY_UPGRADE_AFTER_MS);
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

  /**
   * Put this connection through an ICE restart: a fresh set of servers, a fresh
   * gather, a fresh set of checks, over a rendezvous the caller reopens for it.
   *
   * Shared by the two things that ask for one, which differ only in what a
   * failure costs: a connection that already failed has nothing to keep, and an
   * optional attempt at a better path has a working connection to keep. So this
   * negotiates and reports, and the caller decides.
   */
  const renegotiate = async (phase) => {
    await withinDeadline(openTimeoutMs, async (remaining) => {
      await onFailed();
      if (torn) return;
      diagnostic("restarting", { phase });
      const freshServers = await fetchIceServers();
      ensureActive();
      peer.setConfiguration?.({ iceServers: freshServers });
      await offer(peer, signal, freshServers, { iceRestart: true }, ensureActive);
      await usable(peer, channels, remaining(), diagnostic, (cancel) => (cancelWait = cancel), ensureActive);
    }, (cancel) => (cancelWait = cancel));
  };

  stopWatching = watchForFailure(peer, diagnostic, async () => {
    if (renegotiating) return; // one negotiation at a time; see `attemptDirectPair`
    renegotiating = true;
    restoring = true;
    recovery.begin();
    try {
      await renegotiate("restart");
      if (torn) return;
      // `connected` here is ICE's and DTLS's word. After the bridge restarts,
      // the NEW process answers this offer and the channels still read `open`
      // from the association the old one took with it, so nothing crosses
      // them — and each restart would "land" again, six seconds apart, for
      // ever, with the session never reported lost (#123). Still recovering
      // while this is asked, so the path probe leaves the verdict to it.
      if (!(await carried())) throw blockedBy("not-carried", "the restarted path does not carry this session");
      if (torn) return;
      // The failure watcher is latched for the whole of this restart, carry
      // check included, so a path that failed again while it was asked was
      // heard by nobody. Reported as landed, it would sit failed until the
      // app's own probe severed it (#130).
      if (["failed", "disconnected"].includes(peer.connectionState)) {
        throw blockedBy("failed", "the restarted path failed again before it was confirmed");
      }
      readIceState();
      diagnostic("connected", { phase: "restart" });
      await sampleTransportPath();
      restoring = false;
      recovery.end();
      await onConnected();
      for (const listener of [...restoredListeners]) listener();
    } catch (error) {
      restoring = false;
      diagnostic("restart-failed", { reason: restartFailure(error) });
      tearDown("the ICE restart did not land");
    } finally {
      restoring = false;
      renegotiating = false;
    }
  });
  armDirectPairAttempt();
  const [app, term] = carriers;
  return {
    app,
    term,
    recovery,
    restoring: () => restoring,
    transportPath: () => transportPath,
    /** Hear when this connection starts carrying a different way. Returns the
     *  unsubscribe, which is the only way off. */
    onPathChanged(fn) {
      pathListeners.add(fn);
      return () => pathListeners.delete(fn);
    },
    /** Hear when a failed path carries again after its restart. Returns the
     *  unsubscribe. Not told after the optional direct-pair attempt: that runs
     *  on a path that never failed, so the process behind it cannot have
     *  changed. */
    onRestored(fn) {
      restoredListeners.add(fn);
      return () => restoredListeners.delete(fn);
    },
    close: tearDown,
  };
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
