// The harness's half of the peer connection: how a Node check talks to a bridge.
//
// The browser is live only over the DataChannels (strict P2P transport spec,
// rule 2) and the relay refuses everything that is not `rtc.*` (rule 1), so a
// harness that ran its checks over the relay socket would be checking a path
// the bridge will not carry. This is the harness's half of what the SPA does in
// `spa/src/core/peerLink.js`, `core/carrier.js` and `core/chunk.js`: the same
// two negotiated channels on the same ids, the same 16 KiB chunk wrapper, the
// same 8 MiB reassembly cap.
//
// It is a port and not an import on purpose. `spa/` is a browser bundle with
// its own toolchain, and a harness sharing its source could not catch a change
// that broke the wire — it would break in step. What proves the two copies
// still agree is this one run against the real Rust bridge.
//
// The WebRTC comes from `node-datachannel`'s standard-interface polyfill
// (libdatachannel under it), so what is written here is the same DOM API the
// SPA is written against, and one read tells you whether the two agree.
// `werift` — pure TypeScript, and the first choice for needing no binary — was
// tried first and got as far as DTLS: its SCTP association never completed
// against the bridge's webrtc-rs, so every check would have been blocked on an
// interop bug in the harness's own dependency.

import { RTCPeerConnection } from "node-datachannel/polyfill";

// ------------------------------------------------------------- chunking ---

/** The largest slice of an envelope one DataChannel message carries. Mirrors
 *  CHUNK_BYTES in `spa/src/core/chunk.js` and `bridge/src/rtc/chunk.rs`. */
export const CHUNK_BYTES = 16 * 1024;

/** The largest envelope a reassembly may add up to. Past it the parts are a
 *  peer spending this process's memory, and the channel closes. */
export const MAX_REASSEMBLED_BYTES = 8 * 1024 * 1024;

/** How a receiver tells a part from a whole envelope: the wrapper names `part`
 *  first and an envelope never does. */
const PART_PREFIX = '{"part":';

let nextMessageId = 1;

/** How many bytes the code point at `index` takes in UTF-8. A high surrogate
 *  with a low one behind it is one code point of four bytes spanning two string
 *  units — the only place a naive cut lands inside a character. */
function utf8SizeAt(text, index) {
  const unit = text.charCodeAt(index);
  if (unit < 0x80) return 1;
  if (unit < 0x800) return 2;
  const low = unit >= 0xd800 && unit <= 0xdbff ? text.charCodeAt(index + 1) : 0;
  return low >= 0xdc00 && low <= 0xdfff ? 4 : 3;
}

/** The string units one code point of `size` bytes spans. */
const unitsFor = (size) => (size === 4 ? 2 : 1);

/** The text cut at code-point boundaries, no slice past CHUNK_BYTES of UTF-8. */
function slices(text) {
  const cut = [];
  let start = 0;
  let bytes = 0;
  let index = 0;
  while (index < text.length) {
    const size = utf8SizeAt(text, index);
    if (bytes + size > CHUNK_BYTES) {
      cut.push(text.slice(start, index));
      start = index;
      bytes = 0;
    }
    bytes += size;
    index += unitsFor(size);
  }
  cut.push(text.slice(start));
  return cut;
}

/** One envelope as the messages that carry it: itself when it fits, else its
 *  ordered parts, every part of one envelope sharing an id. */
export function splitEnvelope(envelopeJson) {
  const parts = slices(envelopeJson);
  if (parts.length === 1) return parts;
  const id = nextMessageId++;
  return parts.map((data, index) => JSON.stringify({ part: { id, index, count: parts.length }, data }));
}

/** A reassembly that cannot be finished. Always fatal to the channel it arrived
 *  on: parts carry no way to ask for one again. */
export class ChunkError extends Error {
  constructor(message) {
    super(message);
    this.name = "ChunkError";
  }
}

/** One channel's incoming messages, put back together. One verb: hand it what
 *  arrived and it answers with an envelope, with null, or throws. */
export function createReassembler() {
  let pending = null;

  const due = (part) => {
    const open = pending;
    pending = null;
    if (open) {
      if (open.id !== part.id || open.nextIndex !== part.index) {
        throw new ChunkError(`part ${part.index} of message ${part.id} arrived where part ${open.nextIndex} was due`);
      }
      return open;
    }
    if (part.index !== 0) {
      throw new ChunkError(`part ${part.index} of message ${part.id} arrived where part 0 was due`);
    }
    return { id: part.id, count: part.count, nextIndex: 0, bytes: 0, envelope: "" };
  };

  return {
    accept(text) {
      if (!text.startsWith(PART_PREFIX)) {
        const open = pending;
        pending = null;
        if (open) throw new ChunkError(`a whole message arrived where part ${open.nextIndex} was due`);
        return text;
      }
      const { part, data } = JSON.parse(text);
      if (!part || !Number.isInteger(part.count) || !Number.isInteger(part.index) || part.count === 0 || part.index >= part.count) {
        pending = null;
        throw new ChunkError(`a part that is not one: ${text.slice(0, 64)}`);
      }
      const open = due(part);
      open.bytes += Buffer.byteLength(data, "utf8");
      if (open.bytes > MAX_REASSEMBLED_BYTES) {
        throw new ChunkError(`a message past the ${MAX_REASSEMBLED_BYTES} byte reassembly limit`);
      }
      open.envelope += data;
      open.nextIndex += 1;
      if (open.nextIndex === open.count) return open.envelope;
      pending = open;
      return null;
    },
  };
}

// -------------------------------------------------------------- carriers ---

/** One channel as a wire for envelopes: the chunking, the reassembly and the
 *  close report, and nothing about sessions. Mirrors `channelCarrier` in
 *  `spa/src/core/carrier.js`, minus its `DC_BUFFERED_HIGH` drain — a check only
 *  ever sends small requests, and what the *bridge* sends back is held by the
 *  bridge's own backpressure. A harness that starts pushing megabytes at a
 *  device needs that waiting ported too. */
function channelCarrier(channel) {
  const listeners = new Set();
  const reassembler = createReassembler();

  channel.addEventListener("message", (event) => {
    const text = typeof event.data === "string" ? event.data : event.data.toString("utf8");
    let envelope;
    try {
      const envelopeJson = reassembler.accept(text);
      if (envelopeJson === null) return;
      envelope = JSON.parse(envelopeJson);
    } catch (error) {
      // Parts that do not add up, or parts that add up to something that is
      // not an envelope: neither can be resumed, so the channel goes.
      if (!(error instanceof ChunkError) && !(error instanceof SyntaxError)) throw error;
      channel.close();
      return;
    }
    for (const listener of [...listeners]) listener(envelope);
  });

  return {
    label: channel.label,
    send(envelope) {
      if (channel.readyState !== "open") throw new Error(`the ${channel.label} channel closed`);
      for (const part of splitEnvelope(JSON.stringify(envelope))) channel.send(part);
    },
    onEnvelope(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    close: () => channel.close(),
  };
}

// ------------------------------------------------------------ the upgrade ---

/** The two channels every peer carries, created identically on both sides with
 *  explicit ids so no in-band open handshake is needed. Their mirror is
 *  NEGOTIATED_CHANNELS in `bridge/src/rtc.rs`. */
const NEGOTIATED_CHANNELS = [
  ["app", 0],
  ["term", 1],
];

/** How long the two channels have to open before the upgrade is called off.
 *  There is nothing underneath: a harness that cannot reach the bridge
 *  directly or through TURN within this cannot reach it at all. */
const OPEN_TIMEOUT_MS = 20000;

/** The ICE servers the api mints for this logged-in session — the same route
 *  and the same session cookie the SPA uses. With no `CF_TURN_KEY_*` set (how
 *  the compose stack runs) it answers a STUN-only list, which is all containers
 *  on one network need: they pair host to host. */
export async function fetchIceServers({ apiUrl, cookie }) {
  const response = await fetch(`${apiUrl}/api/rtc/ice-servers`, {
    method: "POST",
    headers: cookie ? { Cookie: cookie } : {},
  });
  if (!response.ok) throw new Error(`ice-servers mint failed: HTTP ${response.status}`);
  return (await response.json()).iceServers;
}

/**
 * Negotiate one peer connection the way the browser does and hand back its two
 * carriers.
 *
 * @param signal `call` on the session whose `rtc.*` rides the rendezvous — the
 *   relay socket, which is the only thing it may carry (rule 1).
 * @param onSignalPush that session's push subscription. The bridge trickles its
 *   candidates as `rtc.ice` pushes.
 */
export async function openPeerLink({ signal, onSignalPush, apiUrl, cookie, openTimeoutMs = OPEN_TIMEOUT_MS }) {
  const iceServers = await fetchIceServers({ apiUrl, cookie });
  const peer = new RTCPeerConnection({ iceServers });
  const channels = NEGOTIATED_CHANNELS.map(([label, id]) =>
    peer.createDataChannel(label, { negotiated: true, id, ordered: true }),
  );
  const unsubscribe = onSignalPush((push) => {
    if (push.type !== "rtc.ice") return;
    peer.addIceCandidate(placed(push.candidate)).catch(() => {
      /* a candidate the peer will not take costs one path, not the link */
    });
  });

  // The bridge builds this session's peer connection when the offer arrives, so
  // a candidate that gets there first is answered "no peer connection" and
  // lost. A browser is saved from that by timing — gathering is slower than the
  // round trip — and a Node implementation on a container's one interface is
  // not: its host candidate is ready before the offer has been sent. So the
  // first ones wait for the answer, and every one after it goes straight out.
  let offered = false;
  const waiting = [];
  const trickle = (candidate) =>
    signal("rtc.ice", { candidate }).catch(() => {
      /* one lost candidate costs a path; the ones that landed still pair */
    });
  peer.addEventListener("icecandidate", (event) => {
    if (!event.candidate) return; // end of gathering says nothing
    const candidate = event.candidate.toJSON ? event.candidate.toJSON() : event.candidate;
    offered ? trickle(candidate) : waiting.push(candidate);
  });

  let torn = false;
  const close = () => {
    if (torn) return;
    torn = true;
    unsubscribe();
    for (const channel of channels) if (channel.readyState === "open") channel.close();
    peer.close();
  };

  try {
    const offer = await peer.createOffer();
    await peer.setLocalDescription(offer);
    const answer = await signal("rtc.offer", { sdp: offer.sdp, ice_servers: iceServers });
    offered = true;
    for (const candidate of waiting.splice(0)) trickle(candidate);
    await peer.setRemoteDescription({ type: "answer", sdp: answer.sdp });
    await bothOpen(peer, channels, openTimeoutMs);
  } catch (error) {
    close();
    throw error;
  }

  const [app, term] = channels.map(channelCarrier);
  return { peer, app, term, close };
}

/** A trickled candidate as `addIceCandidate` will take it.
 *
 * The bridge names its candidates `sdpMid: ""` — webrtc-rs's empty default —
 * and an empty mid is a mid no media section has, so a strict implementation
 * refuses the whole candidate rather than falling back to the index beside it.
 * A browser survives that by accident: it drops the bridge's candidates too,
 * and pairs on the peer-reflexive candidate the bridge's own connectivity
 * checks create. Here the m-line index is what places it, which is what the
 * bridge means.
 */
const placed = (candidate) => (candidate?.sdpMid ? candidate : { ...candidate, sdpMid: undefined });

/** Settle once both channels carry, or fail the upgrade. However it settles it
 *  leaves no deadline ticking and no listener registered. */
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
        settle(() => reject(new Error(`the peer connection ${peer.connectionState} before its channels opened`)));
      }
    };
    const deadline = setTimeout(
      () => settle(() => reject(new Error("the peer connection did not open its channels"))),
      openTimeoutMs,
    );
    peer.addEventListener("connectionstatechange", giveUpOnFailure);
    for (const channel of channels) {
      if (channel.readyState === "open") opened();
      else channel.addEventListener("open", opened);
    }
  });
}

// -------------------------------------------------------------- sessions ---

/** How long a call waits for its answer before it is not coming. */
const CALL_TIMEOUT_MS = 30000;

/**
 * One E2EE session, riding whatever carrier it is handed.
 *
 * `{sessionId, call, onPush, close}` — the same shape whether a DataChannel or
 * the relay socket is underneath, so every check above it is carrier-agnostic:
 * what the peer path changed is the wire, not the protocol. The session's id
 * and key were minted over the rendezvous and outlive it; this never learns
 * how.
 *
 * @param onFrame sees every decrypted payload before the demux, so a check can
 *   assert on the frame the bridge actually sent (an error's `error_code`, a
 *   push's shape) and not only on what `call` resolves to.
 */
export function openCarriedSession({
  carrier,
  transport,
  sessionId,
  sessionKeyB64,
  deviceId,
  onPush = () => {},
  onFrame = () => {},
  timeoutMs = CALL_TIMEOUT_MS,
}) {
  const pending = new Map();
  const pushListeners = new Set([onPush]);
  let requestId = 0;

  carrier.onEnvelope(async (envelope) => {
    let frame;
    try {
      frame = await transport.decryptEnvelope({ sessionKeyB64, envelope });
    } catch {
      return; // a frame written for another session is not this one's to read
    }
    const payload = frame.payload;
    onFrame(payload);
    const answered = payload && payload.id !== undefined ? pending.get(payload.id) : null;
    if (answered) {
      pending.delete(payload.id);
      answered(payload);
      return;
    }
    if (payload && payload.type) for (const listener of [...pushListeners]) listener(payload);
  });

  async function call(method, params = {}, envelopeFields = {}) {
    const id = "r" + ++requestId;
    const answer = new Promise((resolve, reject) => {
      const deadline = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, timeoutMs);
      pending.set(id, (payload) => {
        clearTimeout(deadline);
        payload.ok ? resolve(payload.result) : reject(refusal(payload));
      });
    });
    const envelope = await transport.encryptFrame({
      sessionKeyB64,
      outerFields: { session_id: sessionId, route_to: `device:${deviceId}` },
      frameFields: { frame_type: "data", sender: "client", payload: { method, id, params, ...envelopeFields } },
    });
    carrier.send(envelope);
    return answer;
  }

  return {
    sessionId,
    deviceId,
    call,
    onPush(fn) {
      pushListeners.add(fn);
      return () => pushListeners.delete(fn);
    },
    close: () => carrier.close(),
  };
}

/** The harness's session over the peer connection. The name the transport
 *  spec's QA contract uses; the machinery is shared with the relay signaling
 *  session, which is what a carrier is for. */
export const openPeerSession = openCarriedSession;

/** The bridge said no. From API 1.1 the refusal carries `error_code`,
 *  `retryable` and `details` beside the string; they ride the Error under
 *  their wire names so a check can read them off it. */
export function refusal(payload) {
  const error = new Error(payload.error);
  for (const field of ["error_code", "retryable", "details"]) {
    if (payload[field] !== undefined) error[field] = payload[field];
  }
  return error;
}
