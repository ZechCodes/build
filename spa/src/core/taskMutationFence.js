// Local read authority, deliberately independent of server query bases and the
// durable write journal. A pending or settled mutation invalidates old reads;
// listeners request authoritative refreshes without changing cached totals.
const taskWrites = new Set([
  "tasks.assign", "tasks.attach", "tasks.close", "tasks.comment", "tasks.create",
  "tasks.read_through", "tasks.reopen", "tasks.unwatch", "tasks.update",
  "tasks.waiting_on.add", "tasks.waiting_on.remove", "tasks.watch",
  "tasks.review.act", "tasks.review.close", "tasks.review.complete", "tasks.review.merge",
  "tasks.review.open", "tasks.review.push", "tasks.review.refresh", "tasks.review.reopen",
  "tasks.review.snapshot", "tasks.review.update",
  // Task Inbox actions use generic entity verbs. Their IDs have no guaranteed
  // prefix, so conservatively fence these device-wide. Project deletion owns
  // task deletion; there is no tasks.delete/dismiss/tracking wire method.
  "entity.seen", "entity.dismiss", "entity.mute", "project.delete",
]);

const states = new Map();
const newEpoch = () => Object.freeze({});
function stateFor(deviceId) {
  if (!states.has(deviceId)) states.set(deviceId, { epoch: newEpoch(), pending: 0, listeners: new Set() });
  return states.get(deviceId);
}

export const isTaskMutationMethod = (method) => taskWrites.has(method);
/** Equality is the only operation on an epoch. Context/account reset never
 * reuses one, including when the same paired device is registered again. */
export const taskMutationEpoch = (deviceId) => stateFor(deviceId).epoch;
/** Number of outstanding task mutations; readers admit dispatch only at zero. */
export const taskMutationsPending = (deviceId) => stateFor(deviceId).pending;

export function subscribeTaskMutations(deviceId, listener) {
  const state = stateFor(deviceId);
  state.listeners.add(listener);
  return () => state.listeners.delete(listener);
}

function announce(state, phase) {
  state.epoch = newEpoch();
  const event = { epoch: state.epoch, pending: state.pending, phase };
  for (const listener of state.listeners) {
    try { listener(event); }
    catch { /* A refresh observer must never change the mutation's outcome. */ }
  }
}

/** Start synchronously before even a greeting wait. Preserve the original
 * returned promise/value and synchronous exception, without retrying anything.
 * Rejection can mean an uncertain write, so it fences old reads just as an
 * acknowledgment does. Neither outcome supplies an authoritative query basis. */
export function wrapTaskMutationCall(deviceId, method, dispatch) {
  if (!isTaskMutationMethod(method)) return dispatch();
  const state = stateFor(deviceId);
  state.pending += 1;
  announce(state, "pending");
  const settle = () => {
    if (states.get(deviceId) !== state) return;
    state.pending -= 1;
    announce(state, "settled");
  };
  try {
    const result = dispatch();
    if (result && typeof result.then === "function") void Promise.resolve(result).then(settle, settle);
    else settle();
    return result;
  } catch (error) {
    settle();
    throw error;
  }
}

/** Reconnection changes read authority, while calls already accepted by the
 * previous session remain pending until their original outcome is known. */
export function retargetTaskMutationFence(deviceId) {
  announce(stateFor(deviceId), "session");
}

/** Called by device retirement/account reset. Late settlements hold the old
 * state and cannot change the replacement context or retain its listeners. */
export function retireTaskMutationFence(deviceId) {
  const state = states.get(deviceId);
  if (!state) return;
  states.delete(deviceId);
  state.pending = 0;
  announce(state, "retired");
  state.listeners.clear();
}

export function resetTaskMutationFences() {
  for (const deviceId of [...states.keys()]) retireTaskMutationFence(deviceId);
}
