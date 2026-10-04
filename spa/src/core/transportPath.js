// Which way a peer connection is actually carrying: straight to the machine,
// or through a TURN relay.
//
// Both are "connected" and neither is a fault — a TURN path is what gets a
// browser to a machine behind a network that will not hole-punch — but they are
// not the same connection to be on, and the one thing the reader cannot see is
// which one they got. The bridge writes the same classification on its side
// ("carrying over host/relay candidates"), so the two ends can be read against
// each other.
//
// Pure: it reads a stats report and answers a word. The sampling is
// core/peerLink.js's, which is the only thing holding a peer connection.

export const TURN = "turn";
export const DIRECT = "direct";

/** The candidate type that means "through a relay". Every other type — host,
 *  srflx, prflx — is a path to the machine itself, however it was found. */
const RELAY = "relay";

/** Every entry of a stats report, whatever shape the report came in: the
 *  browser's `RTCStatsReport` is a Map, and a test's fixture is an array of
 *  entries or a plain object of them. */
function entriesOf(stats) {
  if (!stats) return [];
  if (typeof stats.values === "function") return [...stats.values()];
  if (Array.isArray(stats)) return stats;
  return Object.values(stats);
}

/** The pair the connection is running on.
 *
 *  The transport's `selectedCandidatePairId` is authoritative: a browser can
 *  retain old nominated pairs after an ICE restart. Nomination is the fallback
 *  when no transport points to a pair present in this report. */
function selectedPair(entries) {
  const pairs = entries.filter((entry) => entry.type === "candidate-pair");
  const selected = entries
    .filter((entry) => entry.type === "transport")
    .map((transport) => pairs.find((pair) => transport.selectedCandidatePairId && pair.id === transport.selectedCandidatePairId))
    .find(Boolean);
  return selected || pairs.find((pair) => pair.state === "succeeded" && pair.nominated) || null;
}

const candidateById = (entries, id) =>
  entries.find((entry) => entry.id === id && (entry.type === "local-candidate" || entry.type === "remote-candidate"));

/**
 * How this connection is carrying, or null while nothing says yet.
 *
 * Either side being a relay candidate makes it a TURN path: the traffic goes
 * through the relay whichever end asked for it. A pair whose candidates the
 * report does not carry is not guessed at — "not yet known" is a state the
 * reader is told plainly, and a wrong word here would be read as fact.
 */
export function classifyTransportPath(stats) {
  const entries = entriesOf(stats);
  const pair = selectedPair(entries);
  if (!pair) return null;
  const local = candidateById(entries, pair.localCandidateId);
  const remote = candidateById(entries, pair.remoteCandidateId);
  if (!local?.candidateType || !remote?.candidateType) return null;
  return local.candidateType === RELAY || remote.candidateType === RELAY ? TURN : DIRECT;
}
