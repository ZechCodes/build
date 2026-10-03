// Which candidates a peer connection may pair on, and when (task #31).
//
// # The race, and why the browser is where it is lost
//
// ICE nominates the first pair that connects and never re-nominates. A TURN
// allocation is gathered and reachable at once; a host pair on the same Wi-Fi
// waits on mDNS resolution and consent checks first. So on a network where BOTH
// work, the relay pair often answers first, wins for the life of the session,
// and is billed for every byte of it — which is what a reader sees when the
// bubble says "Connected TURN" after a refresh and said "Connected WebRTC"
// before it, on the same Wi-Fi, minutes apart.
//
// The bridge already makes its own agent wait before accepting a relay pair
// (bridge/src/rtc/policy.rs, `relay_min_wait`, 1.5 s). That is not enough on its
// own: the browser is the offerer, so the browser is the CONTROLLING agent, and
// nomination is the controlling agent's. The bridge's wait governs what the
// bridge accepts; the race the reader loses is run in the browser's check list.
//
// # What a browser can actually do about it
//
// There is no API for "prefer a direct pair" — no knob, no ordering hint. The
// one lever is WHEN a candidate enters a check list, and a browser controls that
// in both directions: it chooses when to signal its own candidates, and when to
// hand the remote ones to `addIceCandidate`. So relay candidates are held back a
// short window at both doors, and a viable direct pair gets that long to win.
//
// Within one ICE generation a held candidate is delivered late, not discarded.
// A restart clears stale held candidates before starting its new round.
// TURN is the fallback that makes a symmetric NAT reachable at all, and a
// browser that withheld it would turn a billed connection into no connection.
//
// **Hides** how a candidate's type is read, and the holding. Pure except for the
// timer, which is injected.

/** How long a relay candidate waits behind the direct ones.
 *
 *  The same 1.5 s the bridge's `DEFAULT_RELAY_MIN_WAIT_MS` uses, deliberately:
 *  the two ends are answering one question and a reader comparing the logs
 *  should not have to hold two numbers. Long enough for a host or
 *  server-reflexive pair on an ordinary home network to finish its checks, short
 *  enough that a browser with nothing but TURN to offer is not left waiting. */
export const RELAY_HOLD_MS = 1500;

/** What a relay candidate's type reads as, and the one place the word lives. */
const RELAY = "relay";

/** `typ <type>` inside a candidate line — the SDP attribute, not any other
 *  occurrence of the word. A foundation may be spelled "relay" and mean nothing
 *  by it (RFC 5245 §15.1 puts the type after `typ`). */
const TYP = /(?:^|\s)typ\s+(host|srflx|prflx|relay)(?:\s|$)/;

/**
 * What kind of candidate this is, or `""` when it cannot be read.
 *
 * Two shapes reach this. A local candidate arrives as an `RTCIceCandidate` from
 * the `icecandidate` event and carries `type` as a field. A remote one arrives
 * off the wire as an `RTCIceCandidateInit` — a plain object with an SDP string
 * and no `type` at all — so the string is parsed. The field wins where there is
 * one: a parse must never be able to contradict the browser about its own
 * candidate.
 */
export function candidateTypeOf(candidate) {
  if (!candidate) return "";
  if (typeof candidate.type === "string" && candidate.type) return candidate.type;
  const line = typeof candidate.candidate === "string" ? candidate.candidate : "";
  return line.match(TYP)?.[1] || "";
}

/**
 * Whether this candidate goes through a TURN relay.
 *
 * A candidate that cannot be read is NOT treated as relay. The consequence of
 * guessing wrong runs one way: a direct candidate held back costs 1.5 s, and a
 * relay candidate never sent costs the whole connection.
 */
export const isRelayCandidate = (candidate) => candidateTypeOf(candidate) === RELAY;

/**
 * One door's worth of holding back: direct candidates through at once, relay
 * ones behind them.
 *
 * `deliver` is what the candidate was going to do anyway — signal it, or add it
 * — so this sits in front of a door without knowing which door it is. Both of
 * `peerLink.js`'s use it.
 *
 * Two things bound what this can cost.
 *
 * The window runs from the first RELAY candidate, not from the start of
 * gathering. That is what "1.5 s after the host and server-reflexive ones" means
 * in practice: an agent gathers host candidates in milliseconds and reflexive
 * ones in a fraction of a second, and the TURN allocation lands after both, so
 * measuring from the relay candidate gives the direct pairs the window they were
 * already ahead in.
 *
 * And nothing is held until a direct candidate has actually gone through. A
 * browser with no direct candidate to offer — `iceTransportPolicy: "relay"`, or
 * an agent that gathered nothing usable — has no race to protect, and making it
 * wait 1.5 s would put the whole cost of this on exactly the people TURN exists
 * for. The trade is a relay candidate that arrives BEFORE any direct one, which
 * goes straight through unheld; that ordering is not what an agent does (host
 * first, then reflexive, then the allocation), and paying a certain cost to cover
 * an unlikely ordering is the wrong way round.
 */
export function createRelayHold({
  deliver,
  delayMs = RELAY_HOLD_MS,
  setTimer = (fn, ms) => setTimeout(fn, ms),
  clearTimer = (timer) => clearTimeout(timer),
}) {
  const held = [];
  let timer = null;
  let holding = true;
  let closed = false;
  /** Whether a direct candidate has gone through yet. Until one has, there is no
   *  direct pair for a relay candidate to beat, so there is nothing to hold it
   *  for. */
  let sawDirect = false;

  /** The window is over: everything held goes now, and nothing is held again. */
  const release = () => {
    if (timer !== null) {
      clearTimer(timer);
      timer = null;
    }
    holding = false;
    if (closed) return;
    for (const candidate of held.splice(0)) deliver(candidate);
  };

  return {
    /** One candidate, on its way through this door. */
    offer(candidate) {
      if (closed) return;
      if (!isRelayCandidate(candidate)) {
        sawDirect = true;
        deliver(candidate);
        return;
      }
      if (!holding || !sawDirect) {
        deliver(candidate);
        return;
      }
      held.push(candidate);
      if (timer === null) timer = setTimer(release, delayMs);
    },

    /** Fresh ICE credentials start another nomination race. Stale held
     *  candidates belong to the previous round and must not enter this one. */
    reset() {
      if (closed) return;
      clearTimer(timer);
      timer = null;
      held.length = 0;
      holding = true;
      sawDirect = false;
    },

    /** Stop holding: something has already connected, so the race this was
     *  protecting is decided and a relay candidate costs nothing now. */
    stopHolding: release,

    /** The connection has gone. What is held is dropped rather than delivered —
     *  a candidate handed to a closed peer is an error, not a late arrival. */
    close() {
      closed = true;
      held.length = 0;
      if (timer !== null) {
        clearTimer(timer);
        timer = null;
      }
      holding = false;
    },
  };
}

/** Every entry of a stats report, whatever shape it came in — the browser's
 *  `RTCStatsReport` is a Map, a fixture is an array or a plain object. Kept
 *  beside `transportPath.js`'s copy of this rather than shared, because that
 *  module is about ONE reading and this is about another, and a shared helper
 *  between them would be the only thing they had in common. */
function entriesOf(stats) {
  if (!stats) return [];
  if (typeof stats.values === "function") return [...stats.values()];
  if (Array.isArray(stats)) return stats;
  return Object.values(stats);
}

/**
 * Is there a direct pair worth restarting ICE for?
 *
 * True only when a non-relay pair has ALREADY SUCCEEDED and is not the one the
 * connection is on. That is a deliberately high bar, and it is what makes the
 * one re-nomination attempt safe to make: the browser has already proved that
 * direct pair carries, so the restart is not a gamble on a path that might work
 * — it is a second run at a race the relay merely answered first.
 *
 * A pair that failed its checks, a report with no direct pair in it at all (a
 * symmetric NAT, which is what TURN exists for), and a report that cannot be
 * read all answer false. Disturbing a working relay path on a guess is worse
 * than paying for it.
 */
export const directPairWorthTrying = (stats) => directPairStatus(stats).worthTrying;

/** What a checklist proves, and why no direct pair can be retried yet. Missing
 *  host candidates describe LAN discovery, while pending and failed describe
 *  checks; neither is evidence for disturbing a working TURN connection. */
export function directPairStatus(stats) {
  const entries = entriesOf(stats);
  if (!entries.length) return { worthTrying: false, reason: "stats-unavailable" };
  const candidates = entries.filter((entry) => ["local-candidate", "remote-candidate"].includes(entry.type));
  const typeById = new Map(candidates.map((entry) => [entry.id, entry.candidateType]));
  const directEnd = (id) => Boolean(typeById.get(id)) && typeById.get(id) !== RELAY;
  const directPairs = entries.filter((entry) => entry.type === "candidate-pair"
    && directEnd(entry.localCandidateId) && directEnd(entry.remoteCandidateId));
  if (directPairs.some((pair) => pair.state === "succeeded" && !pair.nominated)) {
    return { worthTrying: true, reason: "direct-pair-succeeded" };
  }
  return { worthTrying: false, reason: missingDirectPairReason(candidates, directPairs) };
}

function missingDirectPairReason(candidates, pairs) {
  if (pairs.some((pair) => ["waiting", "in-progress", "frozen"].includes(pair.state))) return "direct-checks-pending";
  if (pairs.some((pair) => pair.state === "failed")) return "direct-checks-failed";
  const hasHost = (side) => candidates.some((candidate) => candidate.type === side && candidate.candidateType === "host");
  if (!hasHost("local-candidate") || !hasHost("remote-candidate")) return "no-host-candidates";
  return "no-direct-pairs";
}
