// The harness's chunker, against the wire shape the bridge writes.
//
// `web/peer.mjs` carries a port of the SPA's `core/chunk.js`, and a port is a
// second copy of one fact: the envelope wrapper `{"part":{id,index,count},
// "data":…}` that `bridge/src/rtc/chunk.rs` and `spa/src/core/chunk.js` both
// write. These are the checks that the copy still says the same thing, and
// they need no relay, no bridge and no network — `node --test` runs them in the
// qa image before the stack is touched.

import assert from "node:assert/strict";
import test from "node:test";

import { CHUNK_BYTES, MAX_REASSEMBLED_BYTES, createReassembler, splitEnvelope } from "./peer.mjs";

const utf8 = (text) => Buffer.byteLength(text, "utf8");

test("an envelope that fits crosses whole, with no wrapper", () => {
  const envelope = JSON.stringify({ v: 1, ciphertext: "x".repeat(100) });
  assert.deepEqual(splitEnvelope(envelope), [envelope]);
});

test("a big envelope crosses as ordered parts, none past the chunk limit", () => {
  const envelope = JSON.stringify({ ciphertext: "x".repeat(CHUNK_BYTES * 3) });
  const parts = splitEnvelope(envelope);
  assert.ok(parts.length >= 4, `expected several parts, got ${parts.length}`);
  for (const [index, part] of parts.entries()) {
    const wrapper = JSON.parse(part);
    assert.equal(wrapper.part.index, index);
    assert.equal(wrapper.part.count, parts.length);
    assert.ok(utf8(wrapper.data) <= CHUNK_BYTES, `part ${index} is ${utf8(wrapper.data)} bytes`);
  }
});

test("the parts of one envelope reassemble into exactly that envelope", () => {
  const envelope = JSON.stringify({ ciphertext: "ü".repeat(CHUNK_BYTES), nonce: "🔐".repeat(2000) });
  const reassembler = createReassembler();
  const answers = splitEnvelope(envelope).map((part) => reassembler.accept(part));
  assert.equal(answers.at(-1), envelope);
  assert.deepEqual(answers.slice(0, -1), answers.slice(0, -1).map(() => null));
});

test("no part is cut inside a character", () => {
  // A four-byte code point spans two JS string units: a naive cut lands in the
  // middle of it and JSON.stringify writes a lone surrogate no strict parser
  // takes back.
  const envelope = JSON.stringify({ ciphertext: "🔐".repeat(CHUNK_BYTES) });
  for (const part of splitEnvelope(envelope)) {
    assert.equal(JSON.parse(part).data.includes("�"), false);
    assert.equal(JSON.stringify(JSON.parse(part)), part);
  }
});

test("a whole message arriving mid-reassembly is fatal to the channel", () => {
  const reassembler = createReassembler();
  const [first] = splitEnvelope(JSON.stringify({ ciphertext: "x".repeat(CHUNK_BYTES * 2) }));
  assert.equal(reassembler.accept(first), null);
  assert.throws(() => reassembler.accept('{"v":1}'), /whole message arrived/);
});

test("a part out of order is fatal to the channel", () => {
  const reassembler = createReassembler();
  const parts = splitEnvelope(JSON.stringify({ ciphertext: "x".repeat(CHUNK_BYTES * 2) }));
  assert.equal(reassembler.accept(parts[0]), null);
  assert.throws(() => reassembler.accept(parts[0]), /where part 1 was due/);
});

test("a reassembly past the cap is refused rather than bought", () => {
  const reassembler = createReassembler();
  const data = "x".repeat(CHUNK_BYTES);
  const count = Math.ceil(MAX_REASSEMBLED_BYTES / CHUNK_BYTES) + 1;
  assert.throws(() => {
    for (let index = 0; index < count; index++) {
      reassembler.accept(JSON.stringify({ part: { id: 1, index, count }, data }));
    }
  }, new RegExp(`${MAX_REASSEMBLED_BYTES} byte reassembly limit`));
});
