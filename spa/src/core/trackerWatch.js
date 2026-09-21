// Whether this device's bridge knows what watching is (#65).
//
// `issues.watch`, `conversation.watch` and `issues.read_through` all arrive
// with the bridge half (#64), and a control wired to them on an older bridge is
// a control that can only refuse — a client asking a bridge for a verb it has
// never heard of does not get a polite no.
//
// The read mark is why this matters more than a dark button would suggest:
// ungated, it is sent on every open and every scroll to the end, so an older
// bridge would produce a refusal per glance at an issue.
//
// A CAPABILITY, not a version compare. The v1 adapter's own header states the
// rule — "a surface never asks what version the bridge reports, it asks the
// adapter's `capabilities`" — and #64 states the flag in its greeting, beside
// `issues.attachments`. Where the minor still has to answer, for a bridge
// carrying the verbs but predating the flag, it answers once in `bridgeApi/v1`
// `capabilitiesOf` rather than again here. This started life as a copy of #21's version compare, which is
// the shape still to move.
//
// Refused by default, which `bridgeCapabilities` already is: an unknown device,
// a bridge that never greeted and one no adapter claims all read as a
// capabilities object with every flag off. The cost of a wrong no is a control
// not offered yet; the cost of a wrong yes is a refusal in the reader's face
// for something they did not ask to do.

import { bridgeCapabilities } from "./changeEvents.js";

/**
 * Whether this device's bridge carries the watch verbs and the read mark.
 *
 * Per machine: a phone paired to a new bridge and an old one is offered the
 * switch on the first and not the second.
 *
 * `=== true` rather than a truthy test, so a bridge that states something other
 * than a boolean is not read as having claimed anything; optional chaining
 * because an adapter older than the flag has no `issues` group to read.
 *
 * One flag answers for conversations as well as issues, though it is named for
 * the latter: both verbs arrive together in 1.9 (#64).
 */
export function carriesWatching(deviceId) {
  return bridgeCapabilities(deviceId)?.issues?.watching === true;
}
