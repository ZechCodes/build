const MAX_OBJECTS = 128;
const MAX_BYTES = 64 * 1024;
const encoder = new TextEncoder();
const endMarker = (candidate) => candidate === null || candidate?.candidate === null;
const serializedSize = (candidate) => encoder.encode(JSON.stringify(candidate?.toJSON ? candidate.toJSON() : candidate)).byteLength;

/** Reservations survive delivery to the relay hold. Release them only when
 * that hold hands the candidate to native ICE or encrypted signaling. */
export function createPeerIceQueue({ deliver }) {
  const held = [];
  const reservations = new Map();
  let bytes = 0;
  let count = 0;
  let ready = false;
  let ended = false;
  let endSeen = false;
  const clear = () => {
    held.length = 0;
    reservations.clear();
    bytes = 0;
    count = 0;
    ready = false;
    endSeen = false;
  };
  const reserve = (candidate, size) => {
    const sizes = reservations.get(candidate) || [];
    sizes.push(size);
    reservations.set(candidate, sizes);
  };
  return {
    offer(candidate) {
      if (ended) return;
      const end = endMarker(candidate);
      if (end && endSeen) return;
      const size = serializedSize(candidate);
      if (count >= MAX_OBJECTS || bytes + size > MAX_BYTES) throw new Error("pending ICE queue overflow");
      if (end) endSeen = true;
      count += 1;
      bytes += size;
      reserve(candidate, size);
      if (ready) deliver(candidate);
      else held.push(candidate);
    },
    delivered(candidate) {
      const sizes = reservations.get(candidate);
      if (!sizes?.length) return;
      bytes -= sizes.shift();
      count -= 1;
      if (!sizes.length) reservations.delete(candidate);
    },
    open() {
      if (ended) return;
      ready = true;
      for (const candidate of held.splice(0)) deliver(candidate);
    },
    reset: clear,
    close() { ended = true; clear(); },
  };
}
