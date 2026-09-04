// The browser half of the chunk shape. Its mirror is bridge/src/rtc/chunk.rs and
// the two are written from the spec's table, so the boundaries are pinned here
// by the same values.

import { describe, it, expect } from "vitest";
import { CHUNK_BYTES, MAX_REASSEMBLED_BYTES, createReassembler, splitEnvelope } from "../src/core/chunk.js";

const envelopeOf = (bytes) => "e".repeat(bytes);

function reassemble(parts) {
  const reassembler = createReassembler();
  let completed = null;
  for (const part of parts) completed = reassembler.accept(part);
  return completed;
}

describe("splitEnvelope", () => {
  it("sends an envelope that fits as itself", () => {
    const envelope = envelopeOf(CHUNK_BYTES);
    expect(splitEnvelope(envelope)).toEqual([envelope]);
  });

  it("cuts one byte past the limit into two parts that round-trip", () => {
    const envelope = envelopeOf(CHUNK_BYTES + 1);
    const parts = splitEnvelope(envelope);
    expect(parts).toHaveLength(2);
    expect(reassemble(parts)).toBe(envelope);
  });

  it("keeps every part inside one message", () => {
    const envelope = envelopeOf(CHUNK_BYTES * 7 + 13);
    const parts = splitEnvelope(envelope);
    expect(parts).toHaveLength(8);
    for (const part of parts) expect(JSON.parse(part).data.length).toBeLessThanOrEqual(CHUNK_BYTES);
    expect(reassemble(parts)).toBe(envelope);
  });

  it("leads every part with the wrapper a receiver tells parts apart by", () => {
    for (const part of splitEnvelope(envelopeOf(CHUNK_BYTES + 1))) {
      expect(part.startsWith('{"part":')).toBe(true);
    }
  });

  it("round-trips text a cut could land inside", () => {
    const envelope = "é".repeat(CHUNK_BYTES + 1);
    expect(reassemble(splitEnvelope(envelope))).toBe(envelope);
  });
});

describe("createReassembler", () => {
  it("answers with nothing until the last part", () => {
    const parts = splitEnvelope(envelopeOf(CHUNK_BYTES * 2));
    const reassembler = createReassembler();
    expect(reassembler.accept(parts[0])).toBeNull();
    expect(reassembler.accept(parts[1])).toBe(envelopeOf(CHUNK_BYTES * 2));
  });

  it("hands an unchunked message straight back", () => {
    expect(createReassembler().accept('{"version":1}')).toBe('{"version":1}');
  });

  it("throws on a missing part — a reassembly that lost one cannot be resumed", () => {
    const parts = splitEnvelope(envelopeOf(CHUNK_BYTES * 3));
    const reassembler = createReassembler();
    reassembler.accept(parts[0]);
    expect(() => reassembler.accept(parts[2])).toThrow(/part/);
  });

  it("throws on a reassembly that starts mid-message", () => {
    const parts = splitEnvelope(envelopeOf(CHUNK_BYTES * 2));
    expect(() => createReassembler().accept(parts[1])).toThrow(/part/);
  });

  it("throws when another message's part arrives mid-reassembly", () => {
    const mine = splitEnvelope(envelopeOf(CHUNK_BYTES * 2));
    const theirs = splitEnvelope(envelopeOf(CHUNK_BYTES * 2));
    const reassembler = createReassembler();
    reassembler.accept(mine[0]);
    expect(() => reassembler.accept(theirs[1])).toThrow(/part/);
  });

  it("throws when a whole envelope arrives mid-reassembly", () => {
    const parts = splitEnvelope(envelopeOf(CHUNK_BYTES * 2));
    const reassembler = createReassembler();
    reassembler.accept(parts[0]);
    expect(() => reassembler.accept('{"version":1}')).toThrow(/part/);
  });

  it("throws past the frame cap rather than growing on a peer's word", () => {
    const half = Math.floor(MAX_REASSEMBLED_BYTES / 2);
    const oversized = [0, 1, 2].map((index) =>
      JSON.stringify({ part: { id: 9, index, count: 3 }, data: envelopeOf(half) }),
    );
    expect(() => reassemble(oversized)).toThrow(/limit/);
  });

  it("throws on a part that is not one", () => {
    expect(() => createReassembler().accept('{"part":{"id":1},"data":"x"}')).toThrow();
    expect(() => createReassembler().accept('{"part":{"id":1,"index":0,"count":0},"data":"x"}')).toThrow();
  });
});
