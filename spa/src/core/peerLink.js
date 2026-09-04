// The upgrade, whole job in one call.
//
// One peer connection per E2EE session, the browser always the offerer: it
// builds the connection from the ICE servers the api minted, creates the two
// negotiated channels, offers and trickles both ways over the relay carrier,
// and settles once both channels are open. Any failure rejects and leaves the
// caller on the relay with no retry loop — the relay is the fallback, not a
// retry target.
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
 *  within this has a relay session that is already working. */
const OPEN_TIMEOUT_MS = 15000;

/**
 * @param signal `SessionRpc.call` pinned to the relay carrier — every `rtc.*`
 *   RPC rides the relay for the peer's life, so an ICE restart works while the
 *   channels are down.
 * @param fetchIceServers mints a fresh list; called once per offer, including
 *   every ICE restart, which is how expiring TURN credentials are replaced.
 * @param remoteCandidates subscribes to the bridge's trickled candidates:
 *   `(deliver) => unsubscribe`.
 */
export async function openPeerLink({
  signal,
  fetchIceServers,
  remoteCandidates,
  RTCPeerConnectionImpl = globalThis.RTCPeerConnection,
  openTimeoutMs = OPEN_TIMEOUT_MS,
}) {
  const iceServers = await fetchIceServers();
  const peer = new RTCPeerConnectionImpl({ iceServers });
  const channels = NEGOTIATED_CHANNELS.map(([label, id]) =>
    peer.createDataChannel(label, { negotiated: true, id, ordered: true }),
  );
  const carriers = channels.map((channel) => openCarrier({ channel }));
  const unsubscribe = remoteCandidates((candidate) => {
    peer.addIceCandidate(candidate).catch(() => {
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
  } catch (error) {
    tearDown();
    throw error;
  }

  watchForFailure(peer, () =>
    restart(peer, signal, fetchIceServers).catch(() => {
      // A restart that cannot be negotiated is the end of this carrier: the
      // channels go, and the session's switch falls back to the relay.
      tearDown();
    }),
  );

  const [app, term] = carriers;
  return { app, term, close: tearDown };
}

/** One offer/answer round over the relay carrier, ICE servers and all. */
async function offer(peer, signal, iceServers, options) {
  const local = await peer.createOffer(options);
  await peer.setLocalDescription(local);
  const answer = await signal("rtc.offer", { sdp: local.sdp, ice_servers: iceServers });
  await peer.setRemoteDescription({ type: "answer", sdp: answer.sdp });
}

/** Settle once both channels carry, or fail the upgrade. */
function bothOpen(peer, channels, openTimeoutMs) {
  return new Promise((resolve, reject) => {
    let waiting = channels.length;
    const opened = () => {
      waiting -= 1;
      if (waiting === 0) resolve();
    };
    for (const channel of channels) {
      if (channel.readyState === "open") opened();
      else channel.addEventListener("open", opened);
    }
    peer.addEventListener("connectionstatechange", () => {
      if (peer.connectionState === "failed" || peer.connectionState === "closed") {
        reject(new Error(`the peer connection ${peer.connectionState} before its channels opened`));
      }
    });
    setTimeout(() => reject(new Error("the peer connection did not open its channels")), openTimeoutMs);
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
