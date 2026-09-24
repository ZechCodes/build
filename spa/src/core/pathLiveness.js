// What says a peer path is still there, and what it costs to ask.
//
// Two sessions ride one peer connection — the app's and the terminals' — and
// both have to answer "is this path carrying?" from the same evidence. The rule
// lived in terminal/session.js, where it was the terminals' own; the app
// session now asks it too (issue #30), and a rule two callers derive separately
// is a rule that can give two answers about one wire. So it is here, once.
//
// What is NOT shared is the verdict. The terminals treat ICE's word as reason
// enough to keep the path and re-establish only the shells; the app session
// does not, because a path ICE still calls connected while nothing crosses it
// is precisely the fault #30 is about. `whatVouchesFor` reports the evidence;
// each caller decides what the evidence is worth.

/** How long a decrypted frame vouches for the connection.
 *
 *  Every frame for one session rides ONE channel, in order, so a terminal
 *  flooding output queues a ping behind its bytes: pinging a busy stream
 *  measures the backlog, not the connection, and times out on a path that is
 *  plainly alive. Any frame we decrypted is itself proof the bridge is
 *  reachable, so within this window the probe is skipped entirely — busy is not
 *  dead. Only real silence past it is worth a ping. */
export const FRAME_PROOF_OF_LIFE_MS = 4000;

/** How long a probe waits for a pong before it judges. Short on purpose: the
 *  question is whether the wire carries, and a wire that carries answers a
 *  no-op in one round trip. Anything longer is the bridge's queue, which is not
 *  what is being measured. */
export const PING_TIMEOUT_MS = 3000;

/**
 * When this peer last carried anything, for one session's view of it.
 *
 * The maximum of two clocks on purpose: a session's own last decrypted frame,
 * and the last frame that arrived on ANY channel of this peer. The two channels
 * are one path, so a frame on either is proof it is up — without the second, a
 * quiet terminal session beside a busy app session could not tell "nothing is
 * arriving" from "nothing for me".
 *
 * A wire that cannot answer (a test double, a carrier that has gone) is read as
 * silence rather than as an error: the caller's other evidence still applies.
 */
export const peerFrameAt = (rpc, wire) =>
  Math.max(rpc?.lastFrameAt?.() || 0, wire?.peerFrameAt?.() || 0);

/**
 * When this peer last carried anything at all, a part of a frame included.
 *
 * A large envelope crosses as many parts and counts as a frame only once it
 * is whole, so a path carrying one is silent to `peerFrameAt` until its last
 * part lands. A part is still proof the path reaches the peer (#123).
 */
export const peerHeardAt = (rpc, wire) => Math.max(peerFrameAt(rpc, wire), wire?.peerPartAt?.() || 0);

/**
 * What says this path is still there, in the order the evidence is worth
 * anything.
 *
 *   "frames"        a frame arrived on either channel inside the proof window.
 *                   The strongest thing there is: something actually crossed.
 *   "ice-connected" no frames, but the browser's own ICE still holds the path
 *                   open. It runs consent checks every few seconds (RFC 7675)
 *                   and drops `connected` when they stop being answered, so
 *                   this is a direct measurement of the path where an
 *                   application ping measures the path AND the daemon behind
 *                   it. It is also, on its own, not enough: SCTP will keep
 *                   retransmitting into a path ICE calls connected for
 *                   ~105 seconds before it gives up (#30).
 *   "nothing"       neither. Nothing vouches for this wire.
 *
 * `now` is injectable so a test can place the proof window without sleeping.
 */
export function whatVouchesFor(rpc, wire, now = Date.now()) {
  if (now - peerFrameAt(rpc, wire) < FRAME_PROOF_OF_LIFE_MS) return "frames";
  return wire?.peerIsConnected?.() === true ? "ice-connected" : "nothing";
}
