import { openCarrier, peerFrames } from "./carrier.js";
import { classifyTransportPath, TURN } from "./transportPath.js";
import { candidateTypeOf, createRelayHold, directPairStatus, hasLocalHostCandidate, offerHasHostCandidate } from "./iceCandidates.js";
import { recordConnectionDiagnostic } from "./connectionDiagnostics.js";
import { createDirectPairMonitor } from "./directPairMonitor.js";
import { candidateDiagnostic, directPairNoTryReason, safeCandidateReason } from "./rtcDiagnostics.js";

const CHANNELS = [["app", 0], ["term", 1]];
const OPEN_TIMEOUT_MS = 15000;

const blockedBy = (reason, message) => Object.assign(new Error(message), { blockedReason: reason });
const safeState = (state) => (["new", "connecting", "connected", "disconnected", "failed", "closed"].includes(state) ? state : "unknown");
const safeIceState = (state) => (["new", "checking", "connected", "completed", "disconnected", "failed", "closed"].includes(state) ? state : "unknown");
const candidateCounts = () => ({ host: 0, srflx: 0, prflx: 0, relay: 0, unknown: 0 });
const countedCandidateType = (candidate) => {
  const type = candidateTypeOf(candidate);
  return ["host", "srflx", "prflx", "relay"].includes(type) ? type : "unknown";
};
const nativeGatheringHasNoHosts = (round) => round.completed && round.gathered.host === 0 && !round.embeddedHost;
const browserOffersNoHosts = (round, stats) => nativeGatheringHasNoHosts(round) && !hasLocalHostCandidate(stats);

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
  confirmCarried, clientId,
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
  const sampleTransportPath = async (recordUnchanged = true) => {
    try {
      const generation = negotiationGeneration;
      const path = classifyTransportPath(await peerStats());
      if (torn || !path || generation !== negotiationGeneration) return;
      const moved = path !== transportPath;
      transportPath = path;
      // Recorded on every landing, changed or not: "the restart came back on
      // relay again" is worth having in the timeline. Announced only on a
      // change, because that is what a surface has to redraw for.
      if (recordUnchanged || moved) diagnostic("carrying", { path });
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
  let negotiationGeneration = 0;
  let renegotiationDeadline = 0;
  let cancelSignaling = () => {};
  let initialSignalingComplete = Promise.resolve();
  let currentSignalingComplete = initialSignalingComplete;
  const newLocalCandidates = () => ({
    generation: negotiationGeneration, gathered: candidateCounts(), delivered: candidateCounts(), failed: candidateCounts(), pending: new Set(),
    completed: false, embeddedHost: false,
  });
  let localCandidates = newLocalCandidates();
  const rememberLocalOffer = (sdp) => { localCandidates.embeddedHost ||= offerHasHostCandidate(sdp); };
  const reportLocalCandidates = async (phase, round = localCandidates) => {
    const stats = nativeGatheringHasNoHosts(round) ? await peerStats() : null;
    const absence = browserOffersNoHosts(round, stats) ? { reason: "browser-no-host-candidates" } : {};
    diagnostic("local-candidates", {
      generation: round.generation, phase,
      gathered: { ...round.gathered }, delivered: { ...round.delivered }, failed: { ...round.failed }, ...absence,
    });
  };
  /** Whether a failed path is being put right in place: the restart the
   *  failure watcher runs, and the carry check after it. Not the optional
   *  direct-pair attempt, whose path works throughout. What the ring reads to
   *  say the machine is being reconnected to (#123), announced through
   *  `recovery`'s transitions. */
  let restoring = false;
  /** A caller that cannot ask its session is taken at ICE's word, as before. */
  const carried = async () => (confirmCarried ? confirmCarried() : true);
  // One optional restart per peer link. Preserve LAN resolution for this ICE
  // generation regardless of event ordering; consult it only on a steady TURN
  // path. Gathering and nomination can precede the browser's connected event.
  let upgradeAsked = false;
  let mdnsResolvedInGeneration = false;
  let mdnsUnresolvedInGeneration = false;
  let sweepEligibleInGeneration = false;
  let bridgeGeneration = null;
  let bridgeCandidateReason = null;
  let lastNoTryReason = null;
  const clearBridgeGenerationEvidence = () => {
    mdnsResolvedInGeneration = false;
    mdnsUnresolvedInGeneration = false;
    sweepEligibleInGeneration = false;
    bridgeGeneration = null;
    bridgeCandidateReason = null;
    lastNoTryReason = null;
  };
  const belongsToBridgeGeneration = (detail) => {
    if (bridgeGeneration === null) return true;
    return [detail.generation, detail.sweep?.generation].every((generation) => generation === undefined || generation === bridgeGeneration);
  };
  const bridgeUpgradeReason = () => {
    if (mdnsResolvedInGeneration) return "mdns-resolved";
    return sweepEligibleInGeneration && mdnsUnresolvedInGeneration ? "conntrack-sweep" : null;
  };
  const observed = [];
  const observe = (target, type, listener) => {
    target.addEventListener(type, listener);
    observed.push(() => target.removeEventListener(type, listener));
  };
  const ensureActive = () => {
    if (torn) throw blockedBy("failed", "the peer connection closed");
  };
  // Relay candidates are held behind the direct ones at BOTH doors (task #31).
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
  const outboundCandidatesDelivered = (round = localCandidates) => Promise.all([...round.pending]);
  const holdOutbound = createRelayHold({
    deliver: (gathered) => {
      if (torn) return;
      // Held as the RTCIceCandidate — that is the shape whose `type` the hold
      // reads — and serialised only on the way out, because `toJSON` does not
      // carry it.
      const candidate = gathered.toJSON ? gathered.toJSON() : gathered;
      const round = localCandidates;
      const type = countedCandidateType(gathered);
      const delivery = signal("rtc.ice", { candidate }).then(() => {
        round.delivered[type] += 1;
      }).catch((error) => {
        round.failed[type] += 1;
        const reason = safeCandidateReason(error?.details?.reason);
        diagnostic("candidate-failed", { direction: "local", ...(reason ? { reason } : {}) });
      }).finally(() => {
        round.pending.delete(delivery);
        if (peer.iceGatheringState === "complete") reportLocalCandidates("candidate-delivered", round);
      });
      round.pending.add(delivery);
    },
  });
  const receiveCandidateDiagnostic = (push) => {
    const detail = candidateDiagnostic(push);
    if (!detail) return;
    diagnostic("candidate-diagnostics", detail);
    // Ordered on the bound channel before this ICE generation's candidates.
    // It clears any old resolver result queued while recovery was starting.
    if (detail.phase === "generation") {
      clearBridgeGenerationEvidence();
      bridgeGeneration = detail.generation ?? null;
      return;
    }
    if (!belongsToBridgeGeneration(detail)) return;
    bridgeCandidateReason = detail.reason;
    mdnsUnresolvedInGeneration = detail.candidates.mdns_pending > 0 || detail.candidates.mdns_unresolved > 0;
    // The aggregate tracks eligible unresolved ports through completed or
    // expired send windows. A later skipped port cannot erase another port's
    // evidence; resolution and direct selection retire it in the driver.
    if (detail.sweep) sweepEligibleInGeneration = detail.sweep.eligible_unresolved > 0;
    // A newly resolved LAN address warrants fresh browser nomination even
    // when the old browser checklist cannot show that pair as succeeded.
    if (detail.phase === "mdns-resolved" && detail.candidates.mdns_resolved > 0) mdnsResolvedInGeneration = true;
  };
  const pushHandlers = new Map([
    ["rtc.ice", (push) => holdInbound.offer(push.candidate)],
    ["rtc.diagnostics", receiveCandidateDiagnostic],
  ]);
  const unsubscribe = onPush((push) => {
    if (torn) return;
    pushHandlers.get(push.type)?.(push);
  });
  const outgoingCandidate = (event) => {
    if (torn) return;
    if (!event.candidate) {
      localCandidates.completed = true;
      void reportLocalCandidates("gathering-complete");
      return;
    }
    localCandidates.gathered[countedCandidateType(event.candidate)] += 1;
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
    cancelSignaling();
    cancelWait();
    stopWatching();
    directPairMonitor.close();
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

  /** After TURN has been steady for 20 s, keep watching for direct evidence.
   *  A succeeded alternative pair warrants one re-nomination attempt. Late
   *  mDNS resolution also warrants fresh browser checks and nomination. An
   *  eligible unresolved host sweep warrants a fresh generation too: browser
   *  host pairs that exhausted their checks cannot be revived in place.
   *  With neither evidence, the working TURN path is left alone. */
  // One optional upgrade attempt for this peer link's entire lifetime.
  // Recovery reuses the peer and does not renew that budget.
  const mayMonitorDirectPair = () => !torn && !renegotiating && transportPath === TURN;
  const mayTryDirectPair = () => !upgradeAsked && mayMonitorDirectPair();
  const directPairIsWorthTrying = async () => {
    if (!mayTryDirectPair()) return null;
    const generation = negotiationGeneration;
    const stats = await peerStats();
    const status = directPairStatus(stats);
    // Recovery may have started while getStats was pending. It owns this
    // negotiation; a stale checklist must not create a concurrent offer.
    if (!mayTryDirectPair() || generation !== negotiationGeneration) return null;
    if (status.worthTrying) return status.reason;
    const noBrowserHosts = browserOffersNoHosts(localCandidates, stats);
    const discoveryReason = noBrowserHosts ? null : bridgeUpgradeReason();
    if (discoveryReason) return discoveryReason;
    const reason = noBrowserHosts ? "browser-no-host-candidates" : directPairNoTryReason(status.reason, bridgeCandidateReason);
    if (reason !== lastNoTryReason) diagnostic("direct-pair", { state: "none-to-try", reason });
    lastNoTryReason = reason;
    return null;
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
    const pending = error?.blockedReason === "timeout" && frames.connected && peer.iceGatheringState === "gathering";
    diagnostic("direct-pair", {
      state: pending ? "pending" : "failed", reason: error?.blockedReason === "timeout" ? "timeout" : "failed", path: transportPath,
    });
    if (!torn && ["failed", "disconnected"].includes(peer.connectionState)) {
      tearDown("the direct-pair attempt left the path failed");
    }
  };

  /** Let go of what the attempt took, whichever way it went. `onConnected` is
   *  the rendezvous lease `renegotiate` acquired: a lease held after a failed
   *  attempt would keep a relay socket open for the life of the session, which
   *  rule 1 does not allow. */
  const directPairFinished = async () => {
    if (!torn) {
      holdInbound.stopHolding();
      holdOutbound.stopHolding();
      try {
        await withinDeadline(Math.max(1, renegotiationDeadline - Date.now()), () => outboundCandidatesDelivered(),
          (cancel) => (cancelWait = cancel));
      } catch { /* the existing restart deadline also bounds lease cleanup */ }
    }
    renegotiating = false;
    if (torn) return;
    recovery.end();
    await onConnected();
  };

  const attemptDirectPair = async () => {
    const reason = await directPairIsWorthTrying();
    if (!reason || !mayTryDirectPair()) return;
    upgradeAsked = true;
    renegotiating = true;
    diagnostic("direct-pair", { state: "trying", reason });
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

  const monitorDirectPair = async () => {
    if (!upgradeAsked) return attemptDirectPair();
    // An ICE restart may preserve the old connected path while new checks run.
    // Continue reading nomination after the attempt, without issuing another.
    await sampleTransportPath(false);
    if (!torn && transportPath !== TURN) reportDirectPair();
  };
  const directPairMonitor = createDirectPairMonitor({
    check: monitorDirectPair, canCheck: mayMonitorDirectPair, hasAttempted: () => upgradeAsked,
  });

  // Keep signaling while native gathering finishes, independently of app
  // readiness. A late mDNS host candidate otherwise queues behind a closed
  // rendezvous and the bridge can only report that it received no hosts.
  const finishSignaling = async (deadline) => {
    let cancelGathering = () => {};
    let cancelDeadline = () => {};
    const cancel = () => { cancelGathering(); cancelDeadline(); };
    cancelSignaling = cancel;
    const round = localCandidates;
    try {
      await withinDeadline(Math.max(1, deadline - Date.now()), async () => {
        await gatheringComplete(peer, (cancel) => (cancelGathering = cancel), ensureActive);
        await holdOutbound.whenReleased();
        await outboundCandidatesDelivered(round);
      }, (cancel) => (cancelDeadline = cancel));
      reportLocalCandidates("signaling-complete", round);
    } catch (error) {
      reportLocalCandidates(error?.blockedReason === "timeout" ? "timeout" : "cancelled", round);
    } finally {
      cancelGathering();
      if (cancelSignaling === cancel) cancelSignaling = () => {};
    }
  };

  try {
    diagnostic("negotiating", { phase: "initial" });
    const initialDeadline = Date.now() + openTimeoutMs;
    await withinDeadline(openTimeoutMs, async (remaining) => {
      await offer(peer, signal, iceServers, {}, ensureActive, clientId, rememberLocalOffer);
      initialSignalingComplete = finishSignaling(initialDeadline);
      currentSignalingComplete = initialSignalingComplete;
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
    cancelSignaling();
    negotiationGeneration += 1;
    localCandidates = newLocalCandidates();
    renegotiationDeadline = Date.now() + openTimeoutMs;
    let cancelActivity = () => {};
    let negotiationActive = true;
    const ensureNegotiationActive = () => {
      ensureActive();
      if (!negotiationActive) throw blockedBy("failed", "the ICE negotiation ended");
    };
    holdInbound.reset();
    holdOutbound.reset();
    if (phase !== "direct-pair") {
      holdInbound.stopHolding();
      holdOutbound.stopHolding();
    }
    clearBridgeGenerationEvidence();
    try {
      await withinDeadline(openTimeoutMs, async (remaining) => {
        await onFailed();
        ensureNegotiationActive();
        diagnostic("restarting", { phase });
        const freshServers = await fetchIceServers();
        ensureNegotiationActive();
        peer.setConfiguration?.({ iceServers: freshServers });
        await offer(peer, signal, freshServers, { iceRestart: true }, ensureNegotiationActive, clientId, rememberLocalOffer);
        if (phase === "direct-pair") {
          await gatheringComplete(peer, (cancel) => (cancelActivity = cancel), ensureNegotiationActive);
          ensureNegotiationActive();
          await Promise.all([holdInbound.whenReleased(), holdOutbound.whenReleased()]);
          ensureNegotiationActive();
          await outboundCandidatesDelivered();
          ensureNegotiationActive();
        } else {
          currentSignalingComplete = finishSignaling(renegotiationDeadline);
        }
        await usable(peer, channels, remaining(), diagnostic, (cancel) => (cancelActivity = cancel), ensureNegotiationActive);
      }, (cancel) => (cancelWait = () => { cancelActivity(); cancel(); }));
    } finally {
      negotiationActive = false;
      cancelActivity();
    }
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
      await onConnected(currentSignalingComplete);
      for (const listener of [...restoredListeners]) listener();
    } catch (error) {
      restoring = false;
      diagnostic("restart-failed", { reason: restartFailure(error) });
      tearDown("the ICE restart did not land");
    } finally {
      restoring = false;
      renegotiating = false;
      directPairMonitor.start();
    }
  });
  directPairMonitor.start();
  const [app, term] = carriers;
  return {
    app,
    term,
    whenInitialSignalingComplete: () => initialSignalingComplete,
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

const reportLocalOffer = (peer, local, remember) => remember?.(peer.localDescription?.sdp || local.sdp);

async function offer(peer, signal, iceServers, options, ensureActive, clientId, rememberLocalOffer) {
  const local = await peer.createOffer(options);
  ensureActive();
  rememberLocalOffer?.(local.sdp);
  await peer.setLocalDescription(local);
  ensureActive();
  reportLocalOffer(peer, local, rememberLocalOffer);
  const hint = clientId?.();
  const answer = await signal("rtc.offer", { sdp: local.sdp, ice_servers: iceServers, ...(hint ? { client_id: hint } : {}) });
  ensureActive();
  await peer.setRemoteDescription({ type: "answer", sdp: answer.sdp });
  ensureActive();
  reportLocalOffer(peer, local, rememberLocalOffer);
}

/** ICE can stay connected on the old nominated pair during a restart. New
 *  candidates must finish gathering and signaling before its lease is closed. */
function gatheringComplete(peer, registerCancel, ensureActive) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const settle = (answer) => {
      if (settled) return;
      settled = true;
      peer.removeEventListener("icegatheringstatechange", changed);
      peer.removeEventListener("icecandidate", candidate);
      answer();
    };
    const changed = () => {
      try { ensureActive(); } catch (error) { settle(() => reject(error)); return; }
      if (peer.iceGatheringState === "complete") settle(resolve);
    };
    const candidate = (event) => { if (!event.candidate) settle(resolve); };
    registerCancel(() => settle(() => reject(blockedBy("failed", "the peer connection closed"))));
    peer.addEventListener("icegatheringstatechange", changed);
    peer.addEventListener("icecandidate", candidate);
    changed();
  });
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
