// Whether this device's bridge knows what watching is (#65).
//
// The task's watch switch stands from cached state before a greeting. A press
// on an older bridge gets a plain refusal; its optimistic state reverts.
// Automatic read marks remain gated because they are sent on open and scroll,
// without an explicit press, and an older bridge would refuse each one.
//
// A CAPABILITY, not a version compare. The v1 adapter's own header states the
// rule — "a surface never asks what version the bridge reports, it asks the
// adapter's `capabilities`" — and #64 states the flag in its greeting, beside
// `tasks.attachments`. Where the minor still has to answer, for a bridge
// carrying the verbs but predating the flag, it answers once in `bridgeApi/v1`
// `capabilitiesOf` rather than again here. This started life as a copy of #21's version compare, which is
// the shape still to move.
//
// Refused by default, which `bridgeCapabilities` already is: an unknown device,
// a bridge that never greeted and one no adapter claims all read as a
// capabilities object with every flag off.

import { bridgeCapabilities } from "./changeEvents.js";

/**
 * Whether this device's bridge carries automatic read marks.
 *
 * Per machine: a phone paired to a new bridge and an old one sends read marks
 * only to the first.
 *
 * `=== true` rather than a truthy test, so a bridge that states something other
 * than a boolean is not read as having claimed anything; optional chaining
 * because an adapter older than the flag has no `tasks` group to read.
 *
 * One flag answers for conversations as well as tasks, though it is named for
 * the latter: both verbs arrive together in 1.9 (#64).
 */
export function carriesWatching(deviceId) {
  return bridgeCapabilities(deviceId)?.tasks?.watching === true;
}

/**
 * What the switch shows for one task: whether the reader watches it, and how
 * many OTHERS do (which is what `watchTitle` counts).
 *
 * `watched` is the reader's own watch and `trackers` is the agents following
 * it — two fields rather than one mixed list, settled on #64 at 00:38Z: the
 * tracker array is live at 1.5.0 as agent-id strings and
 * `core/trackerAgentTasks.js` matches on it with `includes`, so putting actor
 * objects in it would have stopped matching silently. The two also mean
 * different deliveries — an agent tracker gets a turn started, the reader's
 * watch gets a row in a list.
 *
 * A record carrying neither reads as not watching, the same safe direction the
 * gate takes.
 */
export function watchStateOf(task) {
  const trackers = Array.isArray(task?.trackers) ? task.trackers : [];
  return {
    watching: Boolean(task?.watched),
    watchers: task?.watchers === undefined ? trackers.length : Number(task.watchers) || 0,
  };
}

/// The key core/trackerTimeline.js invents for a record the bridge wrote
/// without an id: `comment-3`, `event-0`. Never a mark.
const INVENTED_KEY = /^(comment|event)-\d+$/;

/**
 * How far this reader has read, out of the timeline as the page holds it.
 *
 * The newest row, whatever kind it is: `tc-…` and `te-…` share one ordering,
 * and #64 compares marks by the ULID after the prefix (`tracker/inbox.rs`
 * `when`), so a comment id is as good a mark as an event id and the field is
 * named `event_id` for the older of the two kinds rather than for the only one
 * it takes. Nothing to mark on an empty timeline.
 *
 * A key the timeline invented is not sent at all. It is `comment-3`, and the
 * bridge would read everything after the first `-` as the instant — `"3"`,
 * which sorts above every real ULID, and its "never move a mark backwards"
 * guard would then refuse every true mark that followed. Unread would stick at
 * whatever it was, for good, and say nothing: the bridge answers that refusal
 * with a quiet no-op and this page swallows read-mark failures by design.
 */
export const readThrough = (rows) => {
  const newest = (rows || []).at(-1)?.key || "";
  return INVENTED_KEY.test(newest) ? "" : newest;
};
