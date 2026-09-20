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
// # The threshold, and why it was not guessed
//
// 1.5.0 — the minor #13 introduces and #20 lands the item at: `versions.json`
// current 1.5.0 and the bridge's API_VERSION 1.5.0, with `issues.track`,
// `issues.untrack`, `issues.for_agent` and this item all at that minor.
//
// It was held at null until that number was reported rather than inferred, and
// the reason is worth keeping: guess 1.5.0, have the verbs land at 1.6.0, and
// every message sent from the issue page to a 1.5.0 bridge is refused whole —
// the exact outage this gate exists to prevent, with the user's own words as
// the casualty. A gate guessed low is worse than no gate at all.

import { bridgeApiVersion } from "./changeEvents.js";
import { compare } from "./bridgeApi/semver.js";

/**
 * The API minor that first accepts a viewing context naming an issue.
 *
 * A bridge below this is told nothing about which issue is open: it would
 * refuse the whole message rather than the item it does not know.
 */
export const ISSUE_CONTEXT_SINCE = "1.5.0";

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
