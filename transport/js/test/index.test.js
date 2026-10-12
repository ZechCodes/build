import test from "node:test";
import assert from "node:assert/strict";
import { python } from "../testing/python.mjs";

import {
  buildCloseFrame,
  createSessionInit,
  decryptEnvelope,
  encryptFrame,
  fingerprintIdentityKey,
  generateIdentityKeypair,
  generateSessionKey,
  importPinnedIdentityKey,
  openSessionAccept,
  ready,
  verifySignedTransportKey
} from "../src/index.js";

test("verified-device helpers round-trip", async () => {
  await ready();
  const identity = await generateIdentityKeypair();
  const pinned = await importPinnedIdentityKey(identity.publicKeyB64);
  assert.equal(pinned.publicKeyB64, identity.publicKeyB64);
  assert.equal((await fingerprintIdentityKey(identity.publicKeyB64)).length, 64);
});

test("deterministic fingerprint matches python", async () => {
  await ready();
  const identity = await generateIdentityKeypair();

  const pythonFingerprint = python({
        action: "fingerprint_identity_key",
        identity_public_key_b64: identity.publicKeyB64
      });
  const expected = pythonFingerprint;
  assert.equal(await fingerprintIdentityKey(identity.publicKeyB64), expected.fingerprint);
});

test("js encryption round-trip works", async () => {
  const sessionKeyB64 = await generateSessionKey();
  const envelope = await encryptFrame({
    sessionKeyB64,
    outerFields: { session_id: "sess-1", route_to: "device-1" },
    frameFields: { frame_type: "data", sender: "client", payload: { ok: true } }
  });

  const frame = await decryptEnvelope({ sessionKeyB64, envelope });
  assert.equal(frame.frame_type, "data");
  assert.deepEqual(frame.payload, { ok: true });

  const closeEnvelope = await buildCloseFrame({
    sessionKeyB64,
    sessionId: "sess-1",
    routeTo: "device-1",
    sender: "client"
  });
  const closeFrame = await decryptEnvelope({ sessionKeyB64, envelope: closeEnvelope });
  assert.equal(closeFrame.frame_type, "close");
});

test("cross-language bootstrap and messaging interop", async () => {
  await ready();

  const pythonProbe = python({ action: "generate_transport_bundle" });
  const transportBundle = pythonProbe;

  await verifySignedTransportKey({
    identityPublicKeyB64: transportBundle.identity_public_key_b64,
    transportPublicKeyB64: transportBundle.transport_public_key_b64,
    signatureB64: transportBundle.signature_b64
  });

  const { sessionKeyB64, sessionInit } = await createSessionInit({
    sessionId: "sess-cross",
    deviceId: "device-cross",
    deviceTransportPublicKeyB64: transportBundle.transport_public_key_b64
  });

  const pythonBootstrap = python({
        action: "open_session_init",
        transport_private_key_b64: transportBundle.transport_private_key_b64,
        session_init: sessionInit
      });
  const bootstrap = pythonBootstrap;
  assert.equal(bootstrap.session_key_b64, sessionKeyB64);

  const pythonAccept = python({
        action: "build_session_accept",
        session_key_b64: sessionKeyB64,
        session_id: "sess-cross",
        route_to: "client-cross"
      });
  const sessionAccept = pythonAccept;
  const openedAccept = await openSessionAccept({ sessionKeyB64, envelope: sessionAccept });
  assert.equal(openedAccept.sessionId, "sess-cross");

  const clientEnvelope = await encryptFrame({
    sessionKeyB64,
    outerFields: { session_id: "sess-cross", route_to: "device-cross" },
    frameFields: { frame_type: "data", sender: "client", payload: { hello: "python" } }
  });

  const pythonDecrypt = python({
        action: "decrypt_envelope",
        session_key_b64: sessionKeyB64,
        envelope: clientEnvelope
      });
  const decryptedByPython = pythonDecrypt;
  assert.deepEqual(decryptedByPython.payload, { hello: "python" });

  const pythonReply = python({
        action: "encrypt_frame",
        session_key_b64: sessionKeyB64,
        outer_fields: { session_id: "sess-cross", route_to: "client-cross" },
        frame_fields: { frame_type: "data", sender: "device", payload: { hello: "js" } }
      });
  const decryptedByJs = await decryptEnvelope({
    sessionKeyB64,
    envelope: pythonReply
  });
  assert.deepEqual(decryptedByJs.payload, { hello: "js" });
});
