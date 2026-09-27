// Whether this device's bridge can be told which task the user is looking at.
//
// This is a gate, and it guards something worse than a missing feature. A
// bridge validates a viewing context's item kinds and REFUSES the ones it does
// not know — and the refusal takes the whole message with it, not just the
// item. So sending `{kind:"task"}` to a bridge that predates it does not lose
// the context; it loses what the user typed.
//
// That is the same shape as the `tasks` push kind, which cost every device
// all of its subscriptions on 2026-09-20 for exactly this reason: a client
// naming something the bridge has never heard of. The rule learned there
// applies here — ask only for what the bridge says it has.
//
// The adapter's tasks.context flag answers this for each greeted device.
// Older bridges derive that flag from the 1.5.0 introduction; names-only
// greetings state it directly, including when a newer bridge withdraws it.

import { bridgeCapabilities } from "./changeEvents.js";

/**
 * Whether this device's bridge accepts the task item.
 *
 * Read the capability of this greeted device. Unknown devices and unsupported
 * bridges answer no; a wrong yes could refuse the user's whole message.
 */
export function carriesTaskContext(deviceId) {
  return Boolean(deviceId) && bridgeCapabilities(deviceId)?.tasks?.context === true;
}

/**
 * The task item for a viewing context, or nothing.
 *
 * Nothing where the bridge cannot take it, and nothing where the task has not
 * been read yet — a page that stamps a half-read task would tell the agent a
 * number with no title behind it.
 */
export function taskContextItem(task, deviceId) {
  if (!task?.id || !task?.title || !carriesTaskContext(deviceId)) return null;
  return { kind: "task", task_id: task.id, number: task.number, title: task.title };
}
