// Sealing, as a bridge does it (planning/v2/Push Content Security Checklist.md
// "The scheme"), for tests that need blobs the fixture does not hold: other
// nonces, other iats, other bridges. The bridge's own sealer is Rust; this one
// is held to the same fixture by test/pushSealed.test.js.

const utf8 = (text) => new TextEncoder().encode(text);

export function b64uEncode(bytes) {
  let binary = "";
  for (const byte of new Uint8Array(bytes)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function b64uDecode(text) {
  const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (text.length % 4)) % 4));
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}

const concat = (...parts) => {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
};

/** Seal `message` (an object, JSON-encoded) to a recipient's raw public key. */
export async function sealForTest({ recipientPublic, sid, kind, entityId, message, nonce = null, ephemeral = null }) {
  const subtle = globalThis.crypto.subtle;
  const ecdh = { name: "ECDH", namedCurve: "P-256" };
  const pair = ephemeral || await subtle.generateKey(ecdh, true, ["deriveBits"]);
  const epk = new Uint8Array(await subtle.exportKey("raw", pair.publicKey));
  const recipient = await subtle.importKey("raw", recipientPublic, ecdh, false, []);
  const shared = await subtle.deriveBits({ name: "ECDH", public: recipient }, pair.privateKey, 256);
  const ikm = await subtle.importKey("raw", shared, "HKDF", false, ["deriveKey"]);
  const key = await subtle.deriveKey(
    { name: "HKDF", hash: "SHA-256", salt: new Uint8Array(0), info: concat(utf8("build-push-v1"), epk, recipientPublic) },
    ikm, { name: "AES-GCM", length: 256 }, false, ["encrypt"],
  );
  const iv = nonce || globalThis.crypto.getRandomValues(new Uint8Array(12));
  const aad = utf8(`build-push-v1\0${sid}\0${kind}\0${entityId}`);
  const plaintext = typeof message === "string" ? utf8(message) : utf8(JSON.stringify(message));
  const sealed = new Uint8Array(await subtle.encrypt({ name: "AES-GCM", iv, additionalData: aad }, key, plaintext));
  return b64uEncode(concat(new Uint8Array([1]), epk, iv, sealed));
}
