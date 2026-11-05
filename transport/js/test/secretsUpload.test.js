import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const sodium = createRequire(import.meta.url)('libsodium-wrappers');
import { sealSecretUpload } from '../src/index.js';

const context = {
  device_id: 'device-test', creator: 'user-test',
  request_id: '557ab895-85e0-4c94-82ad-f16fcb0dd11c',
  name: 'TEST_TOKEN', ttl_seconds: 3600,
  grant: { scope: 'project', project_id: 'project-test' },
  stage: { owner_id: 'owner-test', conversation_id: 'conversation-test', generation: 'generation-test', draft_id: null },
};

async function opened(value = 'synthetic-secret-value') {
  await sodium.ready;
  const keys = sodium.crypto_box_keypair();
  const sealed = await sealSecretUpload({
    deviceTransportPublicKeyB64: sodium.to_base64(keys.publicKey, sodium.base64_variants.ORIGINAL_NO_PADDING),
    context, value,
  });
  const ciphertext = sodium.from_base64(sealed, sodium.base64_variants.ORIGINAL_NO_PADDING);
  const plaintext = sodium.crypto_box_seal_open(ciphertext, keys.publicKey, keys.privateKey);
  return { ciphertext, plaintext };
}

test('secret upload is fixed-size, context-bound sealed JSON', async () => {
  const { ciphertext, plaintext } = await opened();
  assert.equal(ciphertext.length, 32768 + sodium.crypto_box_SEALBYTES);
  assert.equal(plaintext.length, 32768);
  const size = new DataView(plaintext.buffer, plaintext.byteOffset).getUint32(0);
  assert.deepEqual(JSON.parse(sodium.to_string(plaintext.subarray(4, 4 + size))), {
    domain: 'build.secrets.upload', version: 1, context, value_b64: sodium.to_base64(sodium.from_string('synthetic-secret-value'), sodium.base64_variants.ORIGINAL_NO_PADDING),
  });
  assert.ok(plaintext.subarray(4 + size).every(byte => byte === 0));
});

test('secret values preserve UTF-8 and whitespace at the maximum byte size', async () => {
  const value = ' é'.repeat(5461) + ' ';
  const { plaintext } = await opened(value);
  const size = new DataView(plaintext.buffer, plaintext.byteOffset).getUint32(0);
  const valueB64 = JSON.parse(sodium.to_string(plaintext.subarray(4, 4 + size))).value_b64;
  assert.equal(sodium.to_string(sodium.from_base64(valueB64, sodium.base64_variants.ORIGINAL_NO_PADDING)), value);
});

test('secret upload rejects malformed context and out-of-bounds values before sealing', async () => {
  await sodium.ready;
  const keys = sodium.crypto_box_keypair();
  const deviceTransportPublicKeyB64 = sodium.to_base64(keys.publicKey, sodium.base64_variants.ORIGINAL_NO_PADDING);
  for (const value of ['', null, 42, 'bad\0value', 'x'.repeat(16385)]) {
    await assert.rejects(sealSecretUpload({ deviceTransportPublicKeyB64, context, value }), /secret_invalid/);
  }
  for (const change of [{ unknown: true }, { request_id: 'bad' }, { ttl_seconds: 0 }, { ttl_seconds: 2592001 }, { name: 'bad name' }, { grant: { scope: 'world' } }]) {
    await assert.rejects(sealSecretUpload({ deviceTransportPublicKeyB64, context: { ...context, ...change }, value: 'synthetic' }), /secret_invalid/);
  }
});

test('full-size newline and control values fit the fixed frame', async () => {
  for (const value of ['\n'.repeat(16384), '\u0001'.repeat(16384)]) {
    const { plaintext } = await opened(value);
    const size = new DataView(plaintext.buffer, plaintext.byteOffset).getUint32(0);
    const envelope = JSON.parse(sodium.to_string(plaintext.subarray(4, 4 + size)));
    assert.equal(sodium.to_string(sodium.from_base64(envelope.value_b64, sodium.base64_variants.ORIGINAL_NO_PADDING)), value);
  }
});
