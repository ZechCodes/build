// Whether this tab's connections carry right now, asked rather than inferred.
//
// `buildConnectionDiagnostics()` says what the connections did. This asks each
// machine this tab can talk to one question over the path its session rides
// now — the carry check an ICE restart makes before it says `connected`, a
// `ping` answered or a frame arriving — and times the answer. An idle path
// carries nothing, so whether it still would can only be learnt by sending:
// what support asks for when a session reads connected and seems quiet, and
// what scripts/liveness-gate.sh asks after its ICE restart (#131).

import { liveContexts } from "./deviceContexts.js";

/** How long each machine gets to answer. */
export const PROBE_TIMEOUT_MS = 5000;

/** One row per machine that can answer: whether its session is carried, and
 *  how long it took to say. Never rejects; a machine with no session to ask
 *  is not carried. */
export async function probeConnections(timeoutMs = PROBE_TIMEOUT_MS, contexts = liveContexts()) {
  return Promise.all(
    contexts.map(async (context) => {
      const asked = Date.now();
      const carried = context.session?.confirmCarried
        ? (await context.session.confirmCarried(timeoutMs)) === true
        : false;
      return { deviceId: context.deviceId, carried, ms: Date.now() - asked };
    }),
  );
}
