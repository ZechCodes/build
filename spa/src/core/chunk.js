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
 *  Measured in the UTF-8 bytes the wire carries, which is what the negotiated
 *  message limit counts and what bridge/src/rtc/chunk.rs measures. */
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

/** How many bytes the code point at `index` takes in UTF-8. A high surrogate
 *  with a low one behind it is one code point of four bytes spanning two string
 *  units — the only place a naive cut can land inside a character, where it
 *  would leave a half JSON.stringify writes as an escape no strict parser
 *  accepts. */
function utf8SizeAt(text, index) {
  const unit = text.charCodeAt(index);
  if (unit < 0x80) return 1;
  if (unit < 0x800) return 2;
  const low = unit >= 0xd800 && unit <= 0xdbff ? text.charCodeAt(index + 1) : 0;
  if (low >= 0xdc00 && low <= 0xdfff) return 4;
  return 3;
}

/** The string units one code point of `size` bytes spans. */
const unitsFor = (size) => (size === 4 ? 2 : 1);

/** What the wire counts. */
function utf8Length(text) {
  let bytes = 0;
  for (let index = 0; index < text.length; index += unitsFor(utf8SizeAt(text, index))) {
    bytes += utf8SizeAt(text, index);
  }
  return bytes;
}

/** The text cut at code-point boundaries, no slice past CHUNK_BYTES of UTF-8,
 *  and one slice when the whole of it fits. */
function slices(text) {
  const cut = [];
  let start = 0;
  let bytes = 0;
  let index = 0;
  while (index < text.length) {
    const size = utf8SizeAt(text, index);
    if (bytes + size > CHUNK_BYTES) {
      cut.push(text.slice(start, index));
      start = index;
      bytes = 0;
    }
    bytes += size;
    index += unitsFor(size);
  }
  cut.push(text.slice(start));
  return cut;
}

/** One envelope as the messages that carry it: itself, when it fits, else its
 *  ordered parts. Every part of one envelope shares an id, so a receiver that
 *  is handed a part of another message knows before it appends. */
export function splitEnvelope(envelopeJson) {
  const parts = slices(envelopeJson);
  if (parts.length === 1) return parts;
  const id = nextMessageId++;
  return parts.map((data, index) => JSON.stringify({ part: { id, index, count: parts.length }, data }));
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
    return { id: part.id, count: part.count, nextIndex: 0, bytes: 0, envelope: "" };
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
      open.bytes += utf8Length(data);
      if (open.bytes > MAX_REASSEMBLED_BYTES) {
        throw new ChunkError(`a message past the ${MAX_REASSEMBLED_BYTES} byte reassembly limit`);
      }
      open.envelope += data;
      open.nextIndex += 1;
      if (open.nextIndex === open.count) return open.envelope;
      pending = open;
      return null;
    },
  };
}
