import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";
import path from "node:path";
import { python } from "../testing/python.mjs";

const source = process.env.BUILD_SECURE_TRANSPORT_JS;
const binding = await import(source ? pathToFileURL(path.resolve(source, "src/index.js")) : "../src/index.js");
const fixture = process.env.BST_FIXTURES || new URL("../../fixtures/v1.json", import.meta.url);
const bytes = readFileSync(fixture);
const v = JSON.parse(bytes);

test("immutable vector digest", () => {
  if (process.env.BST_FIXTURES) return;
  const expected = readFileSync(new URL("../../fixtures/SHA256SUMS", import.meta.url), "utf8").split(/\s/)[0];
  assert.equal(createHash("sha256").update(bytes).digest("hex"), expected);
});

test("pinned identity, captured bootstrap and accept", async () => {
  await binding.verifySignedTransportKey({ identityPublicKeyB64: v.identity_public,
    transportPublicKeyB64: v.transport_public, signatureB64: v.signature });
  assert.equal(await binding.fingerprintIdentityKey(v.identity_public), v.fingerprint);
  await assert.rejects(binding.verifySignedTransportKey({ identityPublicKeyB64: v.identity_public,
    transportPublicKeyB64: v.transport_public, signatureB64: v.bad_signature }));
  assert.equal(python({ action: "open_session_init", transport_private_key_b64: v.transport_private,
    session_init: v.session_init }).session_key_b64, v.session_key);
  assert.deepEqual(await binding.openSessionAccept({ sessionKeyB64: v.session_key, envelope: v.accept }),
    { sessionId: "vector-session" });
  await assert.rejects(binding.openSessionAccept({ sessionKeyB64: v.session_key,
    envelope: { ...v.accept, session_id: "mismatch" } }));
});

test("deterministic old/new frames and both producers decrypt", async () => {
  for (const frame of v.frames) {
    const envelope = await binding.encryptFrame({ sessionKeyB64: v.session_key,
      outerFields: frame.outer, frameFields: frame.fields, nonceB64: frame.nonce });
    assert.deepEqual(envelope, frame.envelopes.js, frame.name);
    for (const [producer, captured] of Object.entries(frame.envelopes)) {
      assert.deepEqual(await binding.decryptEnvelope({ sessionKeyB64: v.session_key, envelope: captured }),
        frame.plaintext, `${frame.name}/${producer}`);
    }
  }
});

test("rejections and parser baseline", async () => {
  for (const entry of v.envelope_cases) {
    const action = binding.decryptEnvelope({ sessionKeyB64: v.session_key, envelope: entry.envelope });
    if (entry.accepts.js) await action;
    else await assert.rejects(action, undefined, entry.name);
  }
});
