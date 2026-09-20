// Whether this device's bridge can be told which issue the user is looking at.
//
// This is a gate, and it guards something worse than a missing feature. A
// bridge validates a viewing context's item kinds and REFUSES the ones it does
// not know — and the refusal takes the whole message with it, not just the
// item. So sending `{kind:"issue"}` to a bridge that predates it does not lose
// the context; it loses what the user typed.
//
// That is the same shape as the `issues` push kind, which cost every device
// all of its subscriptions on 2026-09-20 for exactly this reason: a client
// naming something the bridge has never heard of. The rule learned there
// applies here — ask only for what the bridge says it has.
//
// # Why this is a version and not a capability
//
// Push kinds are advertised in the greeting (`changes.kinds`), so
// core/trackerPush.js can read the list. Viewing-context kinds are not
// advertised anywhere, so the only signal is the API minor the item landed in
// — which is why #20 puts the item's `since` in the fixture.
//
// # Why the threshold is null
//
// #20 has not landed. Its `since` is "the same minor #13 introduces", and #13
// has not landed either: `versions.json` is at 1.4.0 (`git.changeset_diff`),
// and there are no tracking fixtures. So the number does not exist yet.
//
// Guessing it is the one thing that must not happen here. Guess 1.5.0, have
// #13 land at 1.6.0, and every message sent from the issue page to a 1.5.0
// bridge is refused whole — the exact outage this gate exists to prevent, with
// the user's own words as the casualty.
//
// So the threshold is null until #20 names it, and a null threshold sends
// nothing to anybody. The feature is dark rather than dangerous, and turning
// it on is this one constant. Everything below it is built and tested.

import { bridgeApiVersion } from "./changeEvents.js";
import { compare } from "./bridgeApi/semver.js";

/**
 * The API minor that first accepts a viewing context naming an issue.
 *
 * null means "no bridge does yet". Set it to the minor #20 lands the item at —
 * the same one #13 introduces — and nothing else here has to change.
 */
export const ISSUE_CONTEXT_SINCE = null;

/**
 * Whether this device's bridge accepts the issue item.
 *
 * Read defensively and refused by default: every unknown — no device, no
 * greeting yet, a version that does not parse, a threshold not yet set — answers
 * no. The cost of a wrong yes is the user's message; the cost of a wrong no is
 * an agent that has to be told which issue is open.
 */
export function carriesIssueContext(deviceId, since = ISSUE_CONTEXT_SINCE) {
  if (!since || !deviceId) return false;
  const version = bridgeApiVersion(deviceId);
  if (!version) return false;
  try {
    return compare(version, since) >= 0;
  } catch {
    return false;
  }
}

/**
 * The issue item for a viewing context, or nothing.
 *
 * Nothing where the bridge cannot take it, and nothing where the issue has not
 * been read yet — a page that stamps a half-read issue would tell the agent a
 * number with no title behind it.
 */
export function issueContextItem(issue, deviceId, since = ISSUE_CONTEXT_SINCE) {
  if (!issue?.id || !issue?.title || !carriesIssueContext(deviceId, since)) return null;
  return { kind: "issue", issue_id: issue.id, number: issue.number, title: issue.title };
}
