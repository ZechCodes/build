// Whether a bridge carries the tasks push, asked in one place.
//
// This is a one-line question with a large blast radius, which is why it is a
// module rather than an expression repeated three times.
//
// Every kind in one `changes.subscribe` shares that call's fate, and a bridge
// that does not know a kind refuses the whole call rather than ignore the one
// word. A bridge announcing `changes.refusedKinds` names the kind, and
// `addDesired` (core/changeEvents.js) asks again without it; an older one names
// nothing, and the subscription that asked for `tasks` is lost with it — the
// inbox's `state` and `thread` too. So the kind is not asked for at all where
// the bridge does not say it carries it.
//
// `session.hello` answers the question outright: it advertises `changes.kinds`
// from the bridge's own kind list, so a bridge that carries tasks says so and
// one that predates them does not. A greeting that states nothing readable is
// read as carrying nothing, which is the safe direction — the tracker loses its
// push and fills from the ordered pass instead, and everything else on that
// device goes on working.

import { bridgeCapabilities } from "./changeEvents.js";

/** The wire's name for the tracker's push kind. */
export const TASKS_KIND = "tasks";

/** Whether this device's bridge says its subscriptions carry tasks. Read
 *  defensively: a capability object in a shape this build does not expect must
 *  cost the tracker its push and nothing else. */
export const carriesTasksPush = (deviceId) =>
  (bridgeCapabilities(deviceId)?.changes?.kinds || []).includes(TASKS_KIND);

/**
 * The kinds a tracker surface subscribes to on this device.
 *
 * Empty where the bridge does not carry them, which asks for no subscription at
 * all (`watchChanges` takes a registration naming no kinds as "no subscription,
 * and hear whatever arrives"). The surface keeps its own reads and the ordered
 * pass keeps its cache warm; what it loses is liveness, not correctness.
 */
export const tasksPushKinds = (deviceId) => (carriesTasksPush(deviceId) ? [TASKS_KIND] : []);

/**
 * The inbox subscription's kinds on this device.
 *
 * `tasks` rides with `state` and `thread` rather than on a subscription of its
 * own: it is not a worktree kind, so it is paced by nothing and costs that
 * flush nothing. Only the new kind is gated — the other two are asked for as
 * they always were, because an advertised list that turns out to be incomplete
 * must not be able to take away what already worked.
 */
export const inboxPushKinds = (deviceId) => ["state", "thread", ...tasksPushKinds(deviceId)];
