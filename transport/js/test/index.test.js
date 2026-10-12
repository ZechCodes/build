import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

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

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..", "..");
const pythonExe = path.join(repoRoot, ".venv", "bin", "python");

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

  const pythonFingerprint = spawnSync(
    pythonExe,
    [path.join(repoRoot, "python", "tests", "interop_helper.py")],
    {
      cwd: repoRoot,
      env: { ...process.env, PYTHONPATH: path.join(repoRoot, "python", "src") },
      input: JSON.stringify({
        action: "fingerprint_identity_key",
        identity_public_key_b64: identity.publicKeyB64
      }),
      encoding: "utf8"
    }
  );
  assert.equal(pythonFingerprint.status, 0, pythonFingerprint.stderr);
  const expected = JSON.parse(pythonFingerprint.stdout);
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

  const pythonProbe = spawnSync(
    pythonExe,
    [path.join(repoRoot, "python", "tests", "interop_helper.py")],
    {
      cwd: repoRoot,
      env: { ...process.env, PYTHONPATH: path.join(repoRoot, "python", "src") },
      input: JSON.stringify({ action: "generate_transport_bundle" }),
      encoding: "utf8"
    }
  );
  assert.equal(pythonProbe.status, 0, pythonProbe.stderr);
  const transportBundle = JSON.parse(pythonProbe.stdout);

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

  const pythonBootstrap = spawnSync(
    pythonExe,
    [path.join(repoRoot, "python", "tests", "interop_helper.py")],
    {
      cwd: repoRoot,
      env: { ...process.env, PYTHONPATH: path.join(repoRoot, "python", "src") },
      input: JSON.stringify({
        action: "open_session_init",
        transport_private_key_b64: transportBundle.transport_private_key_b64,
        session_init: sessionInit
      }),
      encoding: "utf8"
    }
  );
  assert.equal(pythonBootstrap.status, 0, pythonBootstrap.stderr);
  const bootstrap = JSON.parse(pythonBootstrap.stdout);
  assert.equal(bootstrap.session_key_b64, sessionKeyB64);

  const pythonAccept = spawnSync(
    pythonExe,
    [path.join(repoRoot, "python", "tests", "interop_helper.py")],
    {
      cwd: repoRoot,
      env: { ...process.env, PYTHONPATH: path.join(repoRoot, "python", "src") },
      input: JSON.stringify({
        action: "build_session_accept",
        session_key_b64: sessionKeyB64,
        session_id: "sess-cross",
        route_to: "client-cross"
      }),
      encoding: "utf8"
    }
  );
  assert.equal(pythonAccept.status, 0, pythonAccept.stderr);
  const sessionAccept = JSON.parse(pythonAccept.stdout);
  const openedAccept = await openSessionAccept({ sessionKeyB64, envelope: sessionAccept });
  assert.equal(openedAccept.sessionId, "sess-cross");

  const clientEnvelope = await encryptFrame({
    sessionKeyB64,
    outerFields: { session_id: "sess-cross", route_to: "device-cross" },
    frameFields: { frame_type: "data", sender: "client", payload: { hello: "python" } }
  });

  const pythonDecrypt = spawnSync(
    pythonExe,
    [path.join(repoRoot, "python", "tests", "interop_helper.py")],
    {
      cwd: repoRoot,
      env: { ...process.env, PYTHONPATH: path.join(repoRoot, "python", "src") },
      input: JSON.stringify({
        action: "decrypt_envelope",
        session_key_b64: sessionKeyB64,
        envelope: clientEnvelope
      }),
      encoding: "utf8"
    }
  );
  assert.equal(pythonDecrypt.status, 0, pythonDecrypt.stderr);
  const decryptedByPython = JSON.parse(pythonDecrypt.stdout);
  assert.deepEqual(decryptedByPython.payload, { hello: "python" });

  const pythonReply = spawnSync(
    pythonExe,
    [path.join(repoRoot, "python", "tests", "interop_helper.py")],
    {
      cwd: repoRoot,
      env: { ...process.env, PYTHONPATH: path.join(repoRoot, "python", "src") },
      input: JSON.stringify({
        action: "encrypt_frame",
        session_key_b64: sessionKeyB64,
        outer_fields: { session_id: "sess-cross", route_to: "client-cross" },
        frame_fields: { frame_type: "data", sender: "device", payload: { hello: "js" } }
      }),
      encoding: "utf8"
    }
  );
  assert.equal(pythonReply.status, 0, pythonReply.stderr);
  const decryptedByJs = await decryptEnvelope({
    sessionKeyB64,
    envelope: JSON.parse(pythonReply.stdout)
  });
  assert.deepEqual(decryptedByJs.payload, { hello: "js" });
});
