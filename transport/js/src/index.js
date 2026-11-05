const PROTOCOL_VERSION = 1;
const NONCE_BYTES = 24;
const SESSION_KEY_BYTES = 32;
const FRAME_TYPES = new Set(["data", "close"]);
const SENDERS = new Set(["client", "device"]);
let sodiumInstance;
let sodiumReadyPromise;

async function ensureReady() {
  if (!sodiumReadyPromise) {
    sodiumReadyPromise = (async () => {
      let sodiumModule;
      try {
        sodiumModule = await import("libsodium-wrappers");
      } catch (error) {
        if (!(typeof process !== "undefined" && process.versions?.node)) {
          throw error;
        }
        const { createRequire } = await import("node:module");
        const require = createRequire(import.meta.url);
        sodiumModule = require("libsodium-wrappers");
      }

      sodiumInstance = sodiumModule.default ?? sodiumModule;
      await sodiumInstance.ready;
      return sodiumInstance;
    })();
  }

  return sodiumReadyPromise;
}

function sodiumLib() {
  if (!sodiumInstance) {
    throw new Error("libsodium is not initialized");
  }
  return sodiumInstance;
}

function requireString(value, fieldName) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${fieldName} must be a non-empty string`);
  }
  return value;
}

function requireObject(value, fieldName) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${fieldName} must be an object`);
  }
  return value;
}

function toBase64(bytes) {
  const sodium = sodiumLib();
  return sodium.to_base64(bytes, sodium.base64_variants.ORIGINAL_NO_PADDING);
}

function fromBase64(value, fieldName = "value") {
  const sodium = sodiumLib();
  requireString(value, fieldName);
  try {
    return sodium.from_base64(value, sodium.base64_variants.ORIGINAL_NO_PADDING);
  } catch (error) {
    throw new Error(`${fieldName} must be valid base64`);
  }
}

function canonicalize(value) {
  if (Array.isArray(value)) {
    return value.map(canonicalize);
  }
  if (value && typeof value === "object") {
    return Object.keys(value)
      .sort()
      .reduce((acc, key) => {
        acc[key] = canonicalize(value[key]);
        return acc;
      }, {});
  }
  return value;
}

function canonicalJsonBytes(value) {
  const sodium = sodiumLib();
  return sodium.from_string(JSON.stringify(canonicalize(value)));
}

function parseJsonBytes(bytes) {
  const sodium = sodiumLib();
  try {
    return JSON.parse(sodium.to_string(bytes));
  } catch (error) {
    throw new Error("ciphertext did not contain valid UTF-8 JSON");
  }
}

function nowRfc3339() {
  return new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
}

function randomUuid() {
  return crypto.randomUUID();
}

function validateSessionKey(sessionKeyB64) {
  const bytes = fromBase64(sessionKeyB64, "sessionKeyB64");
  if (bytes.length !== SESSION_KEY_BYTES) {
    throw new Error("session key must decode to 32 bytes");
  }
  return bytes;
}

function validateEnvelopeShape(envelope) {
  const value = requireObject(envelope, "envelope");
  const version = value.version ?? PROTOCOL_VERSION;
  if (version !== PROTOCOL_VERSION) {
    throw new Error(`unsupported envelope version: ${version}`);
  }

  const sessionId = requireString(value.session_id, "envelope.session_id");
  const routeTo = requireString(value.route_to, "envelope.route_to");
  const nonce = fromBase64(value.nonce, "envelope.nonce");
  const ciphertext = fromBase64(value.ciphertext, "envelope.ciphertext");

  if (nonce.length !== NONCE_BYTES) {
    throw new Error("nonce must decode to 24 bytes");
  }
  if (ciphertext.length === 0) {
    throw new Error("ciphertext must not be empty");
  }

  return { version, sessionId, routeTo, nonce, ciphertext };
}

function validateFrame(frame, expectedSessionId) {
  const value = requireObject(frame, "frame");
  const sessionId = requireString(value.session_id, "frame.session_id");
  const messageId = requireString(value.message_id, "frame.message_id");
  const frameType = requireString(value.frame_type, "frame.frame_type");
  const sender = requireString(value.sender, "frame.sender");
  const createdAt = requireString(value.created_at, "frame.created_at");

  if (sessionId !== expectedSessionId) {
    throw new Error("inner and outer session_id did not match");
  }
  if (!FRAME_TYPES.has(frameType)) {
    throw new Error(`unsupported frame_type: ${frameType}`);
  }
  if (!SENDERS.has(sender)) {
    throw new Error(`unsupported sender: ${sender}`);
  }
  if (!Object.prototype.hasOwnProperty.call(value, "payload")) {
    throw new Error("frame.payload is required");
  }

  return {
    session_id: sessionId,
    message_id: messageId,
    frame_type: frameType,
    sender,
    created_at: createdAt,
    payload: value.payload
  };
}

function secretboxEncrypt(sessionKey, payloadBytes, nonceBytes) {
  const sodium = sodiumLib();
  const nonce = nonceBytes ?? sodium.randombytes_buf(NONCE_BYTES);
  const ciphertext = sodium.crypto_secretbox_easy(payloadBytes, nonce, sessionKey);
  return {
    nonce: toBase64(nonce),
    ciphertext: toBase64(ciphertext)
  };
}

export async function ready() {
  await ensureReady();
}

export async function generateSessionKey() {
  await ensureReady();
  const sodium = sodiumLib();
  return toBase64(sodium.randombytes_buf(SESSION_KEY_BYTES));
}

export async function generateIdentityKeypair() {
  await ensureReady();
  const sodium = sodiumLib();
  const pair = sodium.crypto_sign_keypair();
  return {
    publicKeyB64: toBase64(pair.publicKey),
    privateKeyB64: toBase64(pair.privateKey)
  };
}

export async function importPinnedIdentityKey(publicKeyB64) {
  await ensureReady();
  const sodium = sodiumLib();
  const publicKey = fromBase64(publicKeyB64, "publicKeyB64");
  if (publicKey.length !== sodium.crypto_sign_PUBLICKEYBYTES) {
    throw new Error("identity public key must decode to 32 bytes");
  }
  return {
    publicKeyB64: toBase64(publicKey),
    fingerprint: await fingerprintIdentityKey(publicKeyB64)
  };
}

export async function fingerprintIdentityKey(publicKeyB64) {
  await ensureReady();
  const publicKey = fromBase64(publicKeyB64, "publicKeyB64");
  const digest = await crypto.subtle.digest("SHA-256", publicKey);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export async function verifySignedTransportKey({ identityPublicKeyB64, transportPublicKeyB64, signatureB64 }) {
  await ensureReady();
  const sodium = sodiumLib();
  const identityPublicKey = fromBase64(identityPublicKeyB64, "identityPublicKeyB64");
  const transportPublicKey = fromBase64(transportPublicKeyB64, "transportPublicKeyB64");
  const signature = fromBase64(signatureB64, "signatureB64");

  const isValid = sodium.crypto_sign_verify_detached(signature, transportPublicKey, identityPublicKey);
  if (!isValid) {
    throw new Error("transport key signature verification failed");
  }
}

export async function createSessionInit({ sessionId, deviceId, deviceTransportPublicKeyB64, sessionKeyB64 }) {
  await ensureReady();
  const sodium = sodiumLib();
  const finalSessionId = requireString(sessionId, "sessionId");
  const finalDeviceId = requireString(deviceId, "deviceId");
  const deviceTransportPublicKey = fromBase64(deviceTransportPublicKeyB64, "deviceTransportPublicKeyB64");
  const finalSessionKeyB64 = sessionKeyB64 ?? (await generateSessionKey());
  const sessionKey = validateSessionKey(finalSessionKeyB64);

  const wrapped = sodium.crypto_box_seal(sessionKey, deviceTransportPublicKey);
  return {
    sessionKeyB64: finalSessionKeyB64,
    sessionInit: {
      session_id: finalSessionId,
      device_id: finalDeviceId,
      wrapped_session_key: toBase64(wrapped)
    }
  };
}

export async function openSessionAccept({ sessionKeyB64, envelope }) {
  await ensureReady();
  const sodium = sodiumLib();
  const sessionKey = validateSessionKey(sessionKeyB64);
  const parsedEnvelope = validateEnvelopeShape(envelope);
  let plaintext;
  try {
    plaintext = sodium.crypto_secretbox_open_easy(parsedEnvelope.ciphertext, parsedEnvelope.nonce, sessionKey);
  } catch (error) {
    throw new Error("session_accept decryption failed");
  }

  const payload = parseJsonBytes(plaintext);
  const sessionId = requireString(payload.session_id, "session_accept.session_id");
  if (sessionId !== parsedEnvelope.sessionId) {
    throw new Error("session_accept session_id did not match outer envelope");
  }

  return { sessionId };
}

export async function encryptFrame({ sessionKeyB64, outerFields, frameFields, nonceB64 }) {
  await ensureReady();
  const sessionKey = validateSessionKey(sessionKeyB64);
  const outer = requireObject(outerFields, "outerFields");
  const frame = requireObject(frameFields, "frameFields");

  const sessionId = requireString(outer.session_id, "outerFields.session_id");
  const routeTo = requireString(outer.route_to, "outerFields.route_to");
  const frameType = requireString(frame.frame_type, "frameFields.frame_type");
  const sender = requireString(frame.sender, "frameFields.sender");

  const innerFrame = {
    session_id: sessionId,
    message_id: frame.message_id ?? randomUuid(),
    frame_type: frameType,
    sender,
    created_at: frame.created_at ?? nowRfc3339(),
    payload: frame.payload
  };
  validateFrame(innerFrame, sessionId);

  const nonce = nonceB64 ? fromBase64(nonceB64, "nonceB64") : undefined;
  if (nonce && nonce.length !== NONCE_BYTES) {
    throw new Error("nonceB64 must decode to 24 bytes");
  }

  const encrypted = secretboxEncrypt(sessionKey, canonicalJsonBytes(innerFrame), nonce);
  return {
    version: PROTOCOL_VERSION,
    session_id: sessionId,
    route_to: routeTo,
    nonce: encrypted.nonce,
    ciphertext: encrypted.ciphertext
  };
}

export async function decryptEnvelope({ sessionKeyB64, envelope }) {
  await ensureReady();
  const sodium = sodiumLib();
  const sessionKey = validateSessionKey(sessionKeyB64);
  const parsedEnvelope = validateEnvelopeShape(envelope);
  let plaintext;
  try {
    plaintext = sodium.crypto_secretbox_open_easy(parsedEnvelope.ciphertext, parsedEnvelope.nonce, sessionKey);
  } catch (error) {
    throw new Error("envelope decryption failed");
  }

  return validateFrame(parseJsonBytes(plaintext), parsedEnvelope.sessionId);
}

export async function buildCloseFrame({ sessionKeyB64, sessionId, routeTo, sender, payload = null, nonceB64 }) {
  return encryptFrame({
    sessionKeyB64,
    outerFields: {
      session_id: sessionId,
      route_to: routeTo
    },
    frameFields: {
      frame_type: "close",
      sender,
      payload
    },
    nonceB64
  });
}
