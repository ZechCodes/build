// The upgrade, whole job in one call.
//
// One peer connection per E2EE session, the browser always the offerer: it
// builds the connection from the ICE servers the api minted, creates the two
// negotiated channels, offers and trickles both ways over the rendezvous, and
// settles once both channels are open. Any failure rejects, and there is
// nothing below it: the caller blocks that device (spec rule 3), which is what
// a Retry or the presence poll undoes.
//
// Hides SDP, candidates, the rtc.* shapes, chunking and bufferedAmountLow.

import { openCarrier } from "./carrier.js";

/** The two channels every peer carries, created identically on both sides with
 *  explicit ids so no in-band open handshake is needed (spec §DataChannels).
 *  Their mirror is NEGOTIATED_CHANNELS in bridge/src/rtc.rs. */
const NEGOTIATED_CHANNELS = [
  ["app", 0],
  ["term", 1],
];

/** How long a peer connection has to open both channels before the upgrade is
 *  called off. A browser that cannot reach the device directly or through TURN
 *  within this cannot reach it at all — there is no relay underneath — so this
 *  deadline is what turns "still trying" into a blocked device with a reason
 *  (rule 3), and it must never be an indefinite wait. */
const OPEN_TIMEOUT_MS = 15000;

/**
 * @param signal `SessionRpc.call` pinned to the signaling carrier — every
 *   `rtc.*` RPC rides the rendezvous, so an ICE restart works while the
 *   channels are down.
 * @param fetchIceServers mints a fresh list; called once per offer, including
 *   every ICE restart, which is how expiring TURN credentials are replaced.
 * @param onPush the session's push subscription, `(fn) => unsubscribe`. The
 *   bridge trickles its candidates as pushes, and what an `rtc.ice` push looks
 *   like is stated here and nowhere else.
 * @param onConnected both channels are open — the caller's cue to close the
 *   rendezvous (rule 4). Run again after every successful restart.
 * @param onFailed the connection failed and a restart is about to be offered.
 *   Awaited, because the offer needs a rendezvous and the caller is the one who
 *   reopens it and re-attaches this session's signaling; a caller that cannot
 *   is the end of this link.
 */
export async function openPeerLink({
  signal,
  fetchIceServers,
  onPush,
  onConnected = () => {},
  onFailed = () => {},
  RTCPeerConnectionImpl = globalThis.RTCPeerConnection,
  openTimeoutMs = OPEN_TIMEOUT_MS,
}) {
  const iceServers = await fetchIceServers();
  const peer = new RTCPeerConnectionImpl({ iceServers });
  const channels = NEGOTIATED_CHANNELS.map(([label, id]) =>
    peer.createDataChannel(label, { negotiated: true, id, ordered: true }),
  );
  const carriers = channels.map((channel) => openCarrier({ channel }));
  const unsubscribe = onPush((push) => {
    if (push.type !== "rtc.ice") return;
    peer.addIceCandidate(push.candidate).catch(() => {
      /* a candidate the peer will not take costs one path, not the link */
    });
  });

  let torn = false;
  const tearDown = () => {
    if (torn) return;
    torn = true;
    unsubscribe();
    for (const carrier of carriers) carrier.close();
    peer.close();
    signal("rtc.close", {}).catch(() => {
      /* the bridge reaps the peer with the session either way */
    });
  };

  peer.addEventListener("icecandidate", (event) => {
    if (!event.candidate) return; // end of gathering says nothing
    signal("rtc.ice", { candidate: event.candidate.toJSON ? event.candidate.toJSON() : event.candidate }).catch(() => {
      /* one lost candidate costs a path; the ones that landed still pair */
    });
  });

  try {
    await offer(peer, signal, iceServers, {});
    await bothOpen(peer, channels, openTimeoutMs);
    await onConnected();
  } catch (error) {
    tearDown();
    throw error;
  }

  watchForFailure(peer, async () => {
    try {
      // The rendezvous is closed while a connection is up, so the caller
      // reopens it and re-attaches this session's signaling before the offer
      // that needs it is made.
      await onFailed();
      await restart(peer, signal, fetchIceServers);
      await onConnected();
    } catch {
      // A restart that cannot be negotiated is the end of this link: the
      // channels go, and the device is the caller's to block.
      tearDown();
    }
  });

  const [app, term] = carriers;
  return { app, term, close: tearDown };
}

/** The two ways this deadline ends, named in rule 3's words for a blocked
 *  device: the caller shows one of them over that machine, so which failure it
 *  was is said where the failure is, not guessed at from a message. */
const blockedBy = (reason, message) => Object.assign(new Error(message), { blockedReason: reason });

/** One offer/answer round over the relay carrier, ICE servers and all. */
async function offer(peer, signal, iceServers, options) {
  const local = await peer.createOffer(options);
  await peer.setLocalDescription(local);
  const answer = await signal("rtc.offer", { sdp: local.sdp, ice_servers: iceServers });
  await peer.setRemoteDescription({ type: "answer", sdp: answer.sdp });
}

/** Settle once both channels carry, or fail the upgrade. However it settles it
 *  leaves nothing behind: a deadline still ticking or a state listener still
 *  registered would outlive the answer it was asked for, and the second would
 *  hand every later failure to a promise that is already settled. */
function bothOpen(peer, channels, openTimeoutMs) {
  return new Promise((resolve, reject) => {
    let waiting = channels.length;
    const settle = (answer) => {
      clearTimeout(deadline);
      peer.removeEventListener("connectionstatechange", giveUpOnFailure);
      for (const channel of channels) channel.removeEventListener("open", opened);
      answer();
    };
    const opened = () => {
      waiting -= 1;
      if (waiting === 0) settle(resolve);
    };
    const giveUpOnFailure = () => {
      if (peer.connectionState === "failed" || peer.connectionState === "closed") {
        settle(() => reject(blockedBy("failed", `the peer connection ${peer.connectionState} before its channels opened`)));
      }
    };
    const deadline = setTimeout(
      () => settle(() => reject(blockedBy("timeout", "the peer connection did not open its channels"))),
      openTimeoutMs,
    );
    peer.addEventListener("connectionstatechange", giveUpOnFailure);
    for (const channel of channels) {
      if (channel.readyState === "open") opened();
      else channel.addEventListener("open", opened);
    }
  });
}

/** A live connection that fails gets one restart at a time, never a loop. */
function watchForFailure(peer, onFailed) {
  let restarting = false;
  peer.addEventListener("connectionstatechange", async () => {
    if (peer.connectionState !== "failed" || restarting) return;
    restarting = true;
    await onFailed();
    restarting = false;
  });
}

/** Fresh credentials on the same channels: the peer keeps carrying if the new
 *  candidates pair, and the link closes if they do not. */
async function restart(peer, signal, fetchIceServers) {
  const iceServers = await fetchIceServers();
  await offer(peer, signal, iceServers, { iceRestart: true });
}
