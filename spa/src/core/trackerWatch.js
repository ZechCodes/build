// Whether this device's bridge knows what watching is (#65).
//
// The same gate shape #21 uses for the issue viewing context, and for the same
// reason: a client asking a bridge for a verb it has never heard of does not
// get a polite no. `issues.watch`, `conversation.watch` and `issues.read_through`
// all arrive with the bridge half (#64), and a control wired to them on an
// older bridge is a control that can only refuse.
//
// The read mark is why this matters more than a dark button would suggest:
// ungated, it is sent on every open and every scroll to the end, so an older
// bridge would produce a refusal per glance at an issue.
//
// Refused by default. Every unknown — no device, no greeting yet, a version
// that does not parse — answers no, because the cost of a wrong no is a control
// that is not offered yet and the cost of a wrong yes is a refusal in the
// reader's face for something they did not ask to do.

import { bridgeApiVersion } from "./changeEvents.js";
import { compare } from "./bridgeApi/semver.js";

/**
 * The API minor that first carries watching.
 *
 * 1.9.0, settled on #64: the first bridge roll of that work is 1.8.0, and
 * watching lands in the one after it. Named rather than guessed — a gate
 * guessed low ships a switch that refuses on every bridge below the real one.
 */
export const WATCH_SINCE = "1.9.0";

/**
 * Whether this device's bridge carries the watch verbs and the read mark.
 *
 * Read defensively: an unparseable version, an absent greeting and a threshold
 * nobody has set all answer no.
 */
export function carriesWatching(deviceId, since = WATCH_SINCE) {
  if (!since || !deviceId) return false;
  const version = bridgeApiVersion(deviceId);
  if (!version) return false;
  try {
    return compare(version, since) >= 0;
  } catch {
    return false;
  }
}
