import { beforeAll, describe, expect, it } from "vitest";
import * as transport from "@build/secure-transport";
import {
  acceptPeerAnswer, createChannelBinding, createClearCodec, createPeerOffer,
  DTLS_JSON_PROTOCOL, LEGACY_PROTOCOL, hashSdp, randomPeerToken, sdpFingerprint,
} from "../src/core/peerBinding.js";

const challenge = () => ({ type: "channel_challenge_v1", session_id: "app-session", challenge: token(3) });
const token = (byte) => btoa(String.fromCharCode(...new Uint8Array(32).fill(byte))).replace(/=+$/, "");
const fp = (byte) => Array(32).fill(byte).join(":");
const sdp = (byte = "AB", ending = "\r\n") => ["v=0", `a=fingerprint:sha-256 ${fp(byte)}`, "m=application 9 UDP/DTLS/SCTP webrtc-datachannel", "a=sctp-port:5000", ""].join(ending);
const offerSdp = sdp();
const answerSdp = sdp("CD");
let sessionKeyB64;
beforeAll(async () => { sessionKeyB64 = await transport.generateSessionKey(); });

async function negotiation(overrides = {}) {
  const offer = createPeerOffer({ sdp: offerSdp, negotiation: 1, peerNonce: token(1) });
  const answer = {
    sdp: answerSdp, payload_protocol: DTLS_JSON_PROTOCOL, peer_nonce: offer.peer_nonce,
    peer_token: token(2), negotiation: 1, offer_hash: await hashSdp(offerSdp),
    answer_hash: await hashSdp(answerSdp), offer_fingerprint: "ab".repeat(32), answer_fingerprint: "cd".repeat(32),
    ...overrides,
  };
  return { offer, answer, result: await acceptPeerAnswer({ offerSdp, offer, answer }) };
}

async function binding(role = { channelId: 0, channelLabel: "app" }, options = {}) {
  const { result } = await negotiation();
  return createChannelBinding({ transport, sessionId: "app-session", sessionKeyB64, deviceId: "device-1", transcript: result.transcript, ...role, ...options });
}

async function ack(proof, changes = {}, frameChanges = {}, key = sessionKeyB64) {
  const opened = await transport.decryptEnvelope({ sessionKeyB64, envelope: proof.envelope });
  const envelope = await transport.encryptFrame({ sessionKeyB64: key,
    outerFields: { session_id: "app-session", route_to: "client", ...(frameChanges.outer || {}) },
    frameFields: { frame_type: "data", sender: "device", payload: { ...opened.payload, domain: "channel_bound_v1", binding_id: token(4), ...changes }, ...frameChanges.inner },
  });
  return { type: "channel_bound_v1", session_id: "app-session", envelope };
}

describe("authenticated peer transcript", () => {
  it("advertises explicit compatibility and freezes exact verified SDP facts", async () => {
    const { offer, result } = await negotiation();
    expect(offer.payload_protocols).toEqual([DTLS_JSON_PROTOCOL, LEGACY_PROTOCOL]);
    expect(result.protocol).toBe(DTLS_JSON_PROTOCOL);
    expect(result.transcript).toMatchObject({ protocol: DTLS_JSON_PROTOCOL, peer_nonce: token(1), peer_token: token(2), offer_fingerprint: "ab".repeat(32) });
    expect(Object.isFrozen(result.transcript)).toBe(true);
    expect(await hashSdp(offerSdp)).not.toBe(await hashSdp(sdp("AB", "\n")));
    expect(randomPeerToken()).toMatch(/^[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]$/);
  });

  it.each([
    ["payload_protocol", "unknown"], ["peer_nonce", token(8)], ["peer_nonce", token(1) + "="],
    ["peer_token", "A".repeat(42) + "B"], ["negotiation", 2], ["negotiation", "1"],
    ["offer_hash", "00".repeat(32)], ["answer_hash", "AB".repeat(32)],
    ["offer_fingerprint", "00".repeat(32)], ["answer_fingerprint", "00".repeat(32)],
  ])("rejects altered answer %s=%s", async (field, value) => {
    await expect(negotiation({ [field]: value })).rejects.toThrow();
  });

  it.each([0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, "1"])("rejects invalid negotiation %s", (round) => {
    expect(() => createPeerOffer({ sdp: offerSdp, negotiation: round })).toThrow();
  });

  it("selects legacy only before commitment, with no payload autodetection", async () => {
    const { offer, result } = await negotiation();
    const legacy = await acceptPeerAnswer({ offerSdp, offer, answer: { sdp: answerSdp } });
    expect(legacy).toEqual({ protocol: LEGACY_PROTOCOL, transcript: null });
    expect(await acceptPeerAnswer({ offerSdp, offer, answer: { sdp: answerSdp, payload_protocol: LEGACY_PROTOCOL } })).toEqual(legacy);
    await expect(acceptPeerAnswer({ offerSdp, offer, answer: { sdp: answerSdp }, previous: result })).rejects.toThrow();
    await expect(acceptPeerAnswer({ offerSdp, offer, answer: { payload_protocol: "" } })).rejects.toThrow();
  });

  it("preserves the initial proof transcript on an ICE-only restart", async () => {
    const { result } = await negotiation();
    const restartSdp = offerSdp + "a=ice-ufrag:restart\r\n";
    const offer = createPeerOffer({ sdp: restartSdp, negotiation: 2, peerNonce: token(1) });
    const { answer } = await negotiation({ negotiation: 1 });
    const restart = { ...answer, negotiation: 2, offer_hash: await hashSdp(restartSdp) };
    const next = await acceptPeerAnswer({ offerSdp: restartSdp, offer, answer: restart, previous: result });
    expect(next.transcript).toBe(result.transcript);
    await expect(acceptPeerAnswer({ offerSdp: restartSdp, offer, answer: { ...restart, peer_token: token(8) }, previous: result })).rejects.toThrow();
    await expect(acceptPeerAnswer({ offerSdp: restartSdp, offer, answer: restart, previous: { protocol: LEGACY_PROTOCOL, transcript: null } })).rejects.toThrow();
  });
});

describe("SDP fingerprints", () => {
  it("uses effective application media fingerprint, ignoring other media", () => {
    const media = ["v=0", `a=fingerprint:sha-256 ${fp("11")}`, "m=video 9 UDP/TLS/RTP/SAVPF 96", `a=fingerprint:sha-256 ${fp("22")}`, "m=application 9 UDP/DTLS/SCTP webrtc-datachannel", `a=fingerprint:sha-256 ${fp("AB")}`, ""].join("\r\n");
    expect(sdpFingerprint(media)).toBe("ab".repeat(32));
  });
  it.each([
    "v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n",
    sdp().replace("sha-256", "sha-1"), sdp().replace(fp("AB"), "AB:CD"),
    sdp() + `a=fingerprint:sha-256 ${fp("AB")}\r\na=fingerprint:sha-256 ${fp("11")}\r\n`,
    "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
  ])("rejects missing, unsupported or conflicting application fingerprints", (text) => {
    expect(() => sdpFingerprint(text)).toThrow();
  });
});

describe("one-time role proof", () => {
  it.each([{ channelId: 0, channelLabel: "app" }, { channelId: 1, channelLabel: "term" }])("proves and authenticates its own $channelLabel role", async (role) => {
    const pending = await binding(role);
    const proof = await pending.prove(challenge());
    expect(proof).toMatchObject({ type: "channel_bind_v1", session_id: "app-session" });
    const opened = await transport.decryptEnvelope({ sessionKeyB64, envelope: proof.envelope });
    expect(opened).toMatchObject({ sender: "client", frame_type: "data", payload: { domain: "channel_bind_v1", channel_id: role.channelId, channel_label: role.channelLabel, challenge: token(3) } });
    expect(opened.payload).not.toHaveProperty("negotiation");
    const control = await ack(proof);
    const codec = await pending.acceptAck(control);
    expect(codec.encode({ method: "ping" })).toMatchObject({ binding_id: token(4), seq: 1 });
    await expect(pending.acceptAck(control)).rejects.toThrow();
    await expect(pending.prove(challenge())).rejects.toThrow();
  });

  it.each([
    ["domain", "channel_bind_v1"], ["protocol", LEGACY_PROTOCOL], ["session_id", "other"],
    ["peer_nonce", token(8)], ["peer_token", token(8)], ["challenge", token(8)],
    ["offer_hash", "00".repeat(32)], ["answer_hash", "00".repeat(32)],
    ["offer_fingerprint", "00".repeat(32)], ["answer_fingerprint", "00".repeat(32)],
    ["channel_id", 1], ["channel_label", "term"], ["binding_id", token(4) + "="],
  ])("rejects authenticated ack with wrong %s", async (field, value) => {
    const pending = await binding();
    const proof = await pending.prove(challenge());
    await expect(pending.acceptAck(await ack(proof, { [field]: value }))).rejects.toThrow();
    // Failure consumes the attempt; a subsequent correct ack cannot revive it.
    await expect(pending.acceptAck(await ack(proof))).rejects.toThrow();
  });

  it("rejects reflected proof, wrong sender, close frames, wrong session and wrong key", async () => {
    const corruptions = [
      (proof) => Promise.resolve({ ...proof, type: "channel_bound_v1" }),
      (proof) => ack(proof, {}, { inner: { sender: "client" } }),
      (proof) => ack(proof, {}, { inner: { frame_type: "close" } }),
      (proof) => ack(proof, {}, { outer: { session_id: "other" } }),
      async (proof) => ack(proof, {}, {}, await transport.generateSessionKey()),
      async (proof) => ({ ...await ack(proof), session_id: "other" }),
      async (proof) => { const control = await ack(proof); control.envelope.version = 2; return control; },
    ];
    for (const corrupt of corruptions) {
      const pending = await binding();
      const proof = await pending.prove(challenge());
      await expect(pending.acceptAck(await corrupt(proof))).rejects.toThrow();
    }
  });

  it("rejects ack before proof, oversized controls and mismatched native roles", async () => {
    const pending = await binding();
    await expect(pending.acceptAck({})).rejects.toThrow();
    await expect(binding({ channelId: 0, channelLabel: "term" })).rejects.toThrow();
    for (const changes of [{ type: "channel_bound_v1" }, { session_id: "other" }, { challenge: token(3) + "=" }]) {
      const invalid = await binding();
      await expect(invalid.prove({ ...challenge(), ...changes })).rejects.toThrow();
      await expect(invalid.prove(challenge())).rejects.toThrow();
    }
    const next = await binding();
    const proof = await next.prove(challenge());
    await expect(next.acceptAck({ ...await ack(proof), padding: "x".repeat(65536) })).rejects.toThrow();
  });

  it("cannot finish proof encryption after its parent closes", async () => {
    let resume;
    const delayed = { ...transport, encryptFrame: (fields) => new Promise((resolve) => { resume = () => resolve(transport.encryptFrame(fields)); }) };
    const pending = await binding(undefined, { transport: delayed });
    const proof = pending.prove(challenge());
    pending.close();
    resume();
    await expect(proof).rejects.toThrow();
  });

  it("cannot install a codec after parent retirement during ack decryption", async () => {
    let resume;
    const delayed = { ...transport, decryptEnvelope: (fields) => new Promise((resolve) => { resume = () => resolve(transport.decryptEnvelope(fields)); }) };
    const pending = await binding(undefined, { transport: delayed });
    const proof = await pending.prove(challenge());
    const accepted = pending.acceptAck(await ack(proof));
    pending.close();
    resume();
    await expect(accepted).rejects.toThrow();
  });
});

describe("bound clear codec", () => {
  it("keeps independent exact sequence counters and application payloads", () => {
    const left = createClearCodec(token(4));
    const right = createClearCodec(token(4));
    expect(right.decode(left.encode({ text: "🦀" }))).toEqual({ frame_type: "data", payload: { text: "🦀" } });
    expect(left.encode({}).seq).toBe(2);
    expect(left.decode(right.encode(null, "close"))).toEqual({ frame_type: "close", payload: null });
    expect(() => left.encode({})).toThrow();
    expect(() => right.encode({})).toThrow();
  });
  it.each([
    { seq: 0 }, { seq: 2 }, { seq: 1.5 }, { seq: Number.MAX_SAFE_INTEGER + 1 }, { seq: "1" },
    { binding_id: token(8) }, { protocol: LEGACY_PROTOCOL }, { frame_type: "ping" }, { payload: undefined },
  ])("rejects malformed or transplanted frames %j", (changes) => {
    const codec = createClearCodec(token(4));
    expect(() => codec.decode({ protocol: DTLS_JSON_PROTOCOL, binding_id: token(4), seq: 1, frame_type: "data", payload: {}, ...changes })).toThrow();
  });
  it("retires the codec on malformed input or duplicate sequence", () => {
    const codec = createClearCodec(token(4));
    const wire = codec.encode({});
    expect(() => codec.decode({ ...wire, binding_id: token(8) })).toThrow();
    expect(() => codec.decode(wire)).toThrow();
    expect(() => codec.encode({})).toThrow();
    const duplicate = createClearCodec(token(4));
    duplicate.decode(wire);
    expect(() => duplicate.decode(wire)).toThrow();
    const writer = createClearCodec(token(4));
    expect(() => writer.encode(undefined)).toThrow();
    expect(writer.encode({}).seq).toBe(1);
  });
});
