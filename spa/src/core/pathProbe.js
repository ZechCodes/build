// Is this path there? Asked when an RPC's own deadline stopped being able to
// tell us (issue #30).
//
// # The fault this exists for
//
// A phone's session connected, carried a post to the bridge — the project agent
// processed it, attachment intact — and from that moment nothing came back. The
// admission receipt and the `ok` were written into an SCTP association that was
// retransmitting into a path with no far end, and SCTP does not give up on that
// quickly: the channel closed 105 seconds later. ICE said `connected` the whole
// time, because the browser's consent checks were being answered by a path that
// carried nothing useful. The client saw its 12-second deadline, marked the post
// uncertain, and then waited two minutes for a layer beneath it to notice.
//
// So: when a call burns its whole PATH deadline without even a receipt, stop
// believing the ring and ask the wire one question directly.
//
// # Why this is safe to be blunt about
//
// The lesson of the terminals' own probe (terminal/session.js) is that reading a
// slow pong as death re-mints a session every six seconds: a bridge with eleven
// agents on it can take three seconds to answer over a phone's relayed path
// while the path is perfectly healthy. That probe therefore keeps any path ICE
// vouches for. This one cannot — ICE's word is the thing that failed — so the
// bar is moved to the TRIGGER instead of the verdict, and it is high:
//
//   1. an RPC waited its full path deadline (12 s) with no receipt at all, AND
//   2. no frame arrived on either channel of this peer for 4 s, AND
//   3. one ping went unanswered for 3 s with no frame arriving behind it.
//
// A bridge that is merely busy fails none of these: a receipt takes the call off
// the path deadline (core/sessionRpc.js), and any frame at all on either channel
// vouches for the path. Nineteen seconds of total silence on a connection the
// user is actively using is not a queue.
//
// **Hides** the single-flight, the evidence rule and the diagnostic record. It
// does not tear anything down: `onDead` is the owner's, because what a dead path
// costs is the session's business and not the probe's.

import { PING_TIMEOUT_MS, whatVouchesFor } from "./pathLiveness.js";
import { recordConnectionDiagnostic } from "./connectionDiagnostics.js";

export { PING_TIMEOUT_MS };

/** The diagnostic this writes under. Settings → Diagnostics shows the history
 *  as rows, so the next report from a phone arrives with the probe, what
 *  vouched for the path, and the verdict already in it (issue #30 point 4). */
export const PATH_PROBE_EVENT = "path-probe";

/**
 * @param ping sends one no-op RPC on the wire this session rides, with the
 *   probe's own short deadline. The caller builds it, because which wire and
 *   which deadline are the session's to decide.
 * @param wire the carrier this session is riding, or null when nothing is.
 * @param busy whether something else is already putting this path right — an
 *   ICE restart in flight (core/peerLink.js). A restart keeps the channels open
 *   while it renegotiates, so a call can time out under one on a path that is
 *   about to be fine; and the restart has its own deadline and its own teardown,
 *   so the verdict is already somebody's. The terminals' probe stands down on
 *   the same signal.
 * @param rpc the session's rpc, read only for when it last decrypted a frame.
 * @param onDead the path is not there. Called at most once, with what the
 *   verdict was based on.
 * @param diagnosticId the connection this is about, in the `<deviceId>:<sessionId>`
 *   shape every other diagnostic uses.
 * @param now injectable clock, so a test can place the proof window.
 */
export function createPathProbe({
  ping,
  wire,
  rpc = null,
  busy = () => false,
  onDead = () => {},
  diagnosticId = "peer",
  now = () => Date.now(),
}) {
  /** The probe in flight, so several deadlines firing together ask once. A
   *  second question would measure the first question's queue. */
  let asking = null;
  /** Latched: a path judged dead is judged once. Everything after is the
   *  reconnect's business, and a second verdict would sever a session the
   *  supervisor has already replaced. */
  let settled = false;

  const record = (state, detail) => recordConnectionDiagnostic(diagnosticId, PATH_PROBE_EVENT, { state, ...detail });

  const ask = async (method) => {
    const riding = wire();
    if (busy()) {
      record("renegotiating", { method });
      return "alive";
    }
    const before = whatVouchesFor(rpc, riding, now());
    // A frame on either channel inside the proof window is the strongest
    // evidence there is, and it costs nothing to read: the deadline that fired
    // was this call's backlog, not the path.
    if (before === "frames") {
      record("alive", { method, vouched: before });
      return "alive";
    }
    record("asked", { method, vouched: before });
    const asked = now();
    try {
      await ping();
      return "alive";
    } catch (error) {
      // The pong did not come. One thing can still save the path: a frame that
      // arrived while the ping was out, which says the wire carries and the
      // pong is behind a backlog. ICE's word is deliberately not among them.
      const after = whatVouchesFor(rpc, wire(), now());
      const detail = { method, vouched: after, waitedMs: now() - asked, refusal: error?.message || null };
      if (after === "frames") {
        record("alive", detail);
        return "alive";
      }
      record("dead", detail);
      settled = true;
      onDead(detail);
      return "dead";
    }
  };

  return {
    /**
     * An RPC hit its path deadline. Is the wire under it there?
     *
     * `"alive"` — something vouches for the path, or somebody else is already
     * putting it right; the call's own failure stands and the session is kept.
     * `"dead"` — nothing does, and `onDead` has been told. `"no-wire"` — there is nothing to ask about: no carrier, or the
     * verdict is already in. A session with no carrier is one the switch has
     * already reported idle, and severing it twice is not this module's job.
     *
     * Never rejects: a probe that threw would turn one call's timeout into two
     * failures, and the call's own rejection is the one the caller wants.
     */
    judge(method) {
      if (settled || !wire()) return Promise.resolve("no-wire");
      if (!asking) {
        asking = ask(method).catch(() => "alive").finally(() => {
          asking = null;
        });
      }
      return asking;
    },

    /** This session is over, however it ended: ask nothing more about its
     *  wire. */
    stop() {
      settled = true;
    },
  };
}
