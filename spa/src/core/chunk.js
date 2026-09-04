// Splitting one envelope across a DataChannel's message limit, and putting it
// back together.
//
// Pure: no channel, no I/O, no clock. The wire shape it hides —
// {"part":{"id","index","count"},"data":…} — is the one fact this codebase
// states twice, here and in bridge/src/rtc/chunk.rs, with the spec's table as
// the source both are written from.

/** The largest slice of an envelope one DataChannel message carries. Browsers
 *  cap a single message far above this (Chrome at 256 KiB) and the
 *  SDP-negotiated `a=max-message-size` is what actually binds, so this is the
 *  conservative floor every implementation clears rather than a measured
 *  maximum. An envelope at or under it crosses whole.
 *
 *  Measured in string units: an envelope is JSON of base64 and ids, so a unit
 *  is a byte, and text that is not leaves a slice smaller than the cap rather
 *  than larger. */
export const CHUNK_BYTES = 16 * 1024;

/** The largest envelope a reassembly may add up to — the relay's own frame cap,
 *  so neither carrier accepts what the other would refuse. Past it the parts
 *  are a peer spending this tab's memory, and the channel closes. */
export const MAX_REASSEMBLED_BYTES = 8 * 1024 * 1024;

/** How a receiver tells a part from a whole envelope: the wrapper names `part`
 *  first and an envelope never does. */
const PART_PREFIX = '{"part":';

let nextMessageId = 1;

/**
 * A reassembly that cannot be finished. Always fatal to the channel it arrived
 * on: parts carry no way to ask for one again, so a reassembly that lost one
 * can only be started over on a fresh channel.
 */
export class ChunkError extends Error {
  constructor(message) {
    super(message);
    this.name = "ChunkError";
  }
}

/** One envelope as the messages that carry it: itself, when it fits, else its
 *  ordered parts. Every part of one envelope shares an id, so a receiver that
 *  is handed a part of another message knows before it appends. */
export function splitEnvelope(envelopeJson) {
  if (envelopeJson.length <= CHUNK_BYTES) return [envelopeJson];
  const id = nextMessageId++;
  const slices = [];
  for (let start = 0; start < envelopeJson.length; start += CHUNK_BYTES) {
    slices.push(envelopeJson.slice(start, start + CHUNK_BYTES));
  }
  return slices.map((data, index) =>
    JSON.stringify({ part: { id, index, count: slices.length }, data }),
  );
}

/**
 * One channel's incoming messages, put back together.
 *
 * **Hides** the wrapper and the partial buffer. **Interface** one verb: hand it
 * what arrived and it answers with an envelope, with null, or throws the
 * ChunkError that closes the channel.
 */
export function createReassembler() {
  let pending = null;

  const due = (part) => {
    const open = pending;
    pending = null;
    if (open) {
      if (open.id !== part.id || open.nextIndex !== part.index) {
        throw new ChunkError(`part ${part.index} of message ${part.id} arrived where part ${open.nextIndex} was due`);
      }
      return open;
    }
    if (part.index !== 0) {
      throw new ChunkError(`part ${part.index} of message ${part.id} arrived where part 0 was due`);
    }
    return { id: part.id, count: part.count, nextIndex: 0, envelope: "" };
  };

  return {
    accept(text) {
      if (!text.startsWith(PART_PREFIX)) {
        const open = pending;
        pending = null;
        if (open) throw new ChunkError(`a whole message arrived where part ${open.nextIndex} was due`);
        return text;
      }
      const { part, data } = JSON.parse(text);
      if (!part || !Number.isInteger(part.count) || !Number.isInteger(part.index) || part.count === 0 || part.index >= part.count) {
        pending = null;
        throw new ChunkError(`a part that is not one: ${text.slice(0, 64)}`);
      }
      const open = due(part);
      if (open.envelope.length + data.length > MAX_REASSEMBLED_BYTES) {
        throw new ChunkError(`a message past the ${MAX_REASSEMBLED_BYTES} unit reassembly limit`);
      }
      open.envelope += data;
      open.nextIndex += 1;
      if (open.nextIndex === open.count) return open.envelope;
      pending = open;
      return null;
    },
  };
}
