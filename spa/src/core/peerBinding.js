// Authenticated signaling selects a codec. Only a validated one-time channel
// acknowledgment permits its parent adapter to install that codec on a wire.
// These helpers do not establish relay owner provenance or verify native DTLS;
// those remain the responsibilities of the adapters holding the actual peer.
export const DTLS_JSON_PROTOCOL = "dtls-json-v1";
export const LEGACY_PROTOCOL = "secretbox-v1";
const CONTROL_LIMIT = 64 * 1024;
const encoder = new TextEncoder();
const tupleFields = ["protocol", "session_id", "peer_nonce", "peer_token", "offer_hash", "answer_hash", "offer_fingerprint", "answer_fingerprint", "channel_id", "channel_label", "challenge"];
const transcriptFields = ["offer_hash", "answer_hash", "offer_fingerprint", "answer_fingerprint"];

const fail = (reason) => { throw new Error(`invalid WebRTC binding: ${reason}`); };
const requireObject = (value) => {
  if (!value || typeof value !== "object" || Array.isArray(value)) fail("expected object");
  return value;
};
const requireString = (value) => {
  if (typeof value !== "string" || !value.length) fail("expected nonempty string");
  return value;
};
const requireToken = (value) => {
  if (typeof value !== "string" || !/^[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]$/.test(value)) fail("expected canonical 32-byte token");
  return value;
};
const requireNegotiation = (value) => {
  if (!Number.isSafeInteger(value) || value <= 0) fail("invalid negotiation");
  return value;
};
const equalFields = (actual, expected, fields) => {
  for (const field of fields) if (actual[field] !== expected[field]) fail(`mismatched ${field}`);
};

export function randomPeerToken() {
  return btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(32)))).replace(/=+$/, "");
}

export async function hashSdp(sdp) {
  const digest = await crypto.subtle.digest("SHA-256", encoder.encode(requireString(sdp)));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function normalizeFingerprint(line) {
  const match = /^a=fingerprint:sha-256 ([\da-f]{2}(?::[\da-f]{2}){31})$/i.exec(line);
  if (!match) fail("unsupported or malformed DTLS fingerprint");
  return match[1].replaceAll(":", "").toLowerCase();
}

function oneFingerprint(lines) {
  const values = lines.filter((line) => line.startsWith("a=fingerprint:")).map(normalizeFingerprint);
  if (!values.length || new Set(values).size !== 1) fail("missing or conflicting DTLS fingerprint");
  return values[0];
}

/** Session-level fingerprints are inherited by application media sections.
 * Unrelated audio/video certificates are not the DataChannel certificate. */
export function sdpFingerprint(sdp) {
  const sections = requireString(sdp).split(/\r?\n(?=m=)/).map((section) => section.split(/\r?\n/));
  const session = sections[0][0].startsWith("m=") ? [] : sections.shift();
  const applications = sections.filter(([line]) => /^m=application [1-9]\d* /.test(line));
  if (!applications.length) fail("missing application media");
  const values = applications.map((lines) => oneFingerprint(lines.some((line) => line.startsWith("a=fingerprint:")) ? lines : session));
  if (new Set(values).size !== 1) fail("conflicting application DTLS fingerprints");
  return values[0];
}

export function createPeerOffer({ sdp, negotiation, peerNonce = randomPeerToken() }) {
  sdpFingerprint(sdp);
  return Object.freeze({
    payload_protocols: Object.freeze([DTLS_JSON_PROTOCOL, LEGACY_PROTOCOL]),
    peer_nonce: requireToken(peerNonce), negotiation: requireNegotiation(negotiation),
  });
}

function selectedProtocol(offer, answer, previous) {
  const protocol = Object.hasOwn(answer, "payload_protocol") ? answer.payload_protocol : LEGACY_PROTOCOL;
  if (![DTLS_JSON_PROTOCOL, LEGACY_PROTOCOL].includes(protocol)) fail("unsupported selected protocol");
  if (!offer.payload_protocols.includes(protocol)) fail("protocol not offered");
  if (previous && previous.protocol !== protocol) fail("committed protocol changed");
  return protocol;
}

/** Call before setRemoteDescription. `previous` is the accepted selection of
 * this same native peer, never a predecessor peer's selection. ICE-only
 * restarts validate their new SDP but retain the original binding transcript. */
export async function acceptPeerAnswer({ offerSdp, offer, answer, previous = null }) {
  requireObject(answer);
  requireObject(offer);
  requireNegotiation(offer.negotiation);
  requireToken(offer.peer_nonce);
  const protocol = selectedProtocol(offer, answer, previous);
  if (protocol === LEGACY_PROTOCOL) return Object.freeze({ protocol, transcript: null });
  equalFields(answer, offer, ["peer_nonce", "negotiation"]);
  const transcript = Object.freeze({
    protocol, peer_nonce: requireToken(answer.peer_nonce), peer_token: requireToken(answer.peer_token),
    negotiation: requireNegotiation(answer.negotiation),
    offer_hash: await hashSdp(offerSdp), answer_hash: await hashSdp(answer.sdp),
    offer_fingerprint: sdpFingerprint(offerSdp), answer_fingerprint: sdpFingerprint(answer.sdp),
  });
  equalFields(answer, transcript, transcriptFields);
  if (previous) equalFields(transcript, previous.transcript, ["peer_nonce", "peer_token", "offer_fingerprint", "answer_fingerprint"]);
  return Object.freeze({ protocol, transcript: previous?.transcript || transcript });
}

function validateRole(channelId, channelLabel) {
  if (new Map([[0, "app"], [1, "term"]]).get(channelId) !== channelLabel) fail("wrong channel role");
  if (![0, 1].includes(channelId)) fail("invalid channel id");
}

function validateControl(control, type, sessionId) {
  requireObject(control);
  if (encoder.encode(JSON.stringify(control)).byteLength > CONTROL_LIMIT) fail("oversized binding control");
  if (control.type !== type || control.session_id !== sessionId) fail("wrong binding control");
}

function proofTuple(transcript, sessionId, channelId, channelLabel, challenge) {
  if (transcript.protocol !== DTLS_JSON_PROTOCOL) fail("proof requires clear selection");
  return {
    protocol: transcript.protocol, session_id: sessionId,
    peer_nonce: transcript.peer_nonce, peer_token: transcript.peer_token,
    offer_hash: transcript.offer_hash, answer_hash: transcript.answer_hash,
    offer_fingerprint: transcript.offer_fingerprint, answer_fingerprint: transcript.answer_fingerprint,
    channel_id: channelId, channel_label: channelLabel, challenge,
  };
}

/** One attempt only. Parent owns the finite bind deadline and checks actual
 * channel identity/current allocation around these asynchronous operations.
 * Pass the full challenge control so another session's challenge cannot be
 * accidentally answered. A rejected ack permanently consumes this attempt. */
export function createChannelBinding({ transport, sessionId, sessionKeyB64, deviceId, transcript, channelId, channelLabel }) {
  validateRole(channelId, channelLabel);
  requireString(sessionId);
  requireString(deviceId);
  let state = "initial";
  let tuple;
  return {
    async prove(control) {
      if (state !== "initial") fail("challenge already consumed");
      state = "proving";
      validateControl(control, "channel_challenge_v1", sessionId);
      tuple = proofTuple(transcript, sessionId, channelId, channelLabel, requireToken(control.challenge));
      const envelope = await transport.encryptFrame({ sessionKeyB64,
        outerFields: { session_id: sessionId, route_to: `device:${deviceId}` },
        frameFields: { frame_type: "data", sender: "client", payload: { domain: "channel_bind_v1", ...tuple } },
      });
      if (state !== "proving") fail("binding attempt ended");
      state = "pending";
      return { type: "channel_bind_v1", session_id: sessionId, envelope };
    },
    async acceptAck(control) {
      if (state !== "pending") fail("no pending proof");
      state = "consumed";
      validateControl(control, "channel_bound_v1", sessionId);
      const frame = await transport.decryptEnvelope({ sessionKeyB64, envelope: control.envelope });
      equalFields(frame, { session_id: sessionId, sender: "device", frame_type: "data" }, ["session_id", "sender", "frame_type"]);
      requireObject(frame.payload);
      equalFields(frame.payload, { domain: "channel_bound_v1", ...tuple }, ["domain", ...tupleFields]);
      if (state !== "consumed") fail("binding attempt ended");
      state = "bound";
      return createClearCodec(requireToken(frame.payload.binding_id));
    },
    close() { state = "closed"; },
  };
}

function validateClearFrame(frame, bindingId, sequence) {
  requireObject(frame);
  equalFields(frame, { protocol: DTLS_JSON_PROTOCOL, binding_id: bindingId, seq: sequence }, ["protocol", "binding_id", "seq"]);
  requireNegotiation(frame.seq);
  if (!["data", "close"].includes(frame.frame_type)) fail("invalid clear frame type");
  if (frame.payload === undefined) fail("missing clear payload");
}

/** Framing only; possession of a codec is not owner authority. Parent creates
 * it after ack validation and calls encode inside its serialized writer,
 * after cancellation checks. Any malformed inbound frame retires the codec. */
export function createClearCodec(bindingId) {
  requireToken(bindingId);
  let outgoing = 1;
  let incoming = 1;
  let closed = false;
  const ensureOpen = () => { if (closed) fail("clear codec closed"); };
  return {
    encode(payload, frameType = "data") {
      ensureOpen();
      const frame = { protocol: DTLS_JSON_PROTOCOL, binding_id: bindingId, seq: outgoing, frame_type: frameType, payload };
      validateClearFrame(frame, bindingId, outgoing);
      if (outgoing === Number.MAX_SAFE_INTEGER || frameType === "close") closed = true;
      else outgoing += 1;
      return frame;
    },
    decode(frame) {
      ensureOpen();
      try { validateClearFrame(frame, bindingId, incoming); }
      catch (error) { closed = true; throw error; }
      if (incoming === Number.MAX_SAFE_INTEGER || frame.frame_type === "close") closed = true;
      else incoming += 1;
      return { frame_type: frame.frame_type, payload: frame.payload };
    },
    close() { closed = true; },
  };
}
