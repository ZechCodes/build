const MAX_OBJECTS = 128;
const MAX_BYTES = 64 * 1024;
const encoder = new TextEncoder();
const serializedSize = (candidate) => encoder.encode(JSON.stringify(candidate?.toJSON ? candidate.toJSON() : candidate)).byteLength;
const candidateValue = (value) => value?.type === "rtc.ice" ? value.candidate : value;
export const isIceEndMarker = (value) => candidateValue(value) === null || [null, ""].includes(candidateValue(value)?.candidate);
const classifyCandidate = (candidate) => {
  const size = serializedSize(candidate);
  if (size > MAX_BYTES) throw new Error("pending ICE queue overflow");
  const end = isIceEndMarker(candidate);
  const value = end ? normalizedMarker(candidate) : candidate;
  return { original: candidate, size, value, end, markerKey: end ? JSON.stringify([value, candidateValue(candidate) === null ? "global" : "generation"]) : null };
};
const normalizedMarker = (value) => value?.type === "rtc.ice" ? { ...value, candidate: null } : null;

/** Each completion lease owns one reservation in one generation. Native ICE
 * and signaling must release it only after their asynchronous work settles.
 * The end marker stays reserved until all earlier work has completed. */
export function createPeerIceQueue({ deliver, accept = () => true }) {
  const held = [];
  const entries = new Set();
  const earlyMarkers = new Set();
  let generation = 0;
  let bytes = 0;
  let ready = false;
  let ended = false;
  let endSeen = false;
  let marker = null;
  const handoff = (entry) => deliver(entry.value, entry.release, entry.original);
  const flushMarker = () => {
    if (!ready || !marker || entries.size !== 1) return;
    const entry = marker;
    marker = null;
    handoff(entry);
  };
  const reserve = ({ value, size, markerKey, original }) => {
    const epoch = generation;
    const entry = { value, original, markerKey, release: () => {
      if (epoch !== generation || !entries.delete(entry)) return;
      bytes -= size;
      flushMarker();
    } };
    entries.add(entry);
    bytes += size;
    return entry;
  };
  const dispatch = (entry) => {
    earlyMarkers.delete(entry.markerKey);
    if (!accept(entry.original)) return entry.release();
    if (!isIceEndMarker(entry.value)) return handoff(entry);
    if (endSeen) return entry.release();
    endSeen = true;
    marker = entry;
    flushMarker();
  };
  const clear = () => {
    generation += 1;
    held.length = 0;
    entries.clear();
    earlyMarkers.clear();
    bytes = 0;
    ready = false;
    endSeen = false;
    marker = null;
  };
  const duplicate = (entry) => entry.end && (endSeen || earlyMarkers.has(entry.markerKey));
  const ensureBudget = (size) => {
    if (entries.size >= MAX_OBJECTS || bytes + size > MAX_BYTES) throw new Error("pending ICE queue overflow");
  };
  return {
    offer(candidate) {
      if (ended) return;
      if (ready && !accept(candidate)) return;
      const classified = classifyCandidate(candidate);
      if (duplicate(classified)) return;
      ensureBudget(classified.size);
      const entry = reserve(classified);
      if (ready) dispatch(entry);
      else { if (entry.markerKey !== null) earlyMarkers.add(entry.markerKey); held.push(entry); }
    },
    open() {
      if (ended) return;
      ready = true;
      for (const entry of held.splice(0)) dispatch(entry);
    },
    hasPending: () => entries.size > 0,
    reset: clear,
    close() { ended = true; clear(); },
  };
}
