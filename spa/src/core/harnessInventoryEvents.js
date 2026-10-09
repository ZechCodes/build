// A machine's harness inventory moved (#434): its bridge says
// `harnesses.changed` with the revision it moved to, and whoever holds that
// machine's inventory asks `harnesses.list` again, writes the answer to the
// cache and redraws from it. This is only the routing: the inventory reader
// registers per device, and a revision no newer than what it already painted
// is its to ignore.

/** Each device's listeners, for the `harnesses.changed` its bridge pushes. */
const listenersByDevice = new Map();

/** Hear `deviceId`'s `harnesses.changed` revisions from now on. Returns the
 *  unsubscribe. */
export function onHarnessesChanged(deviceId, listener) {
  const listeners = listenersByDevice.get(deviceId) || new Set();
  listeners.add(listener);
  listenersByDevice.set(deviceId, listeners);
  return () => {
    listeners.delete(listener);
    if (!listeners.size) listenersByDevice.delete(deviceId);
  };
}

/** A machine's bridge says its inventory is at `revision` now. */
export function harnessesChangedOn(deviceId, revision) {
  if (!Number.isSafeInteger(revision) || revision < 0) return;
  for (const listener of listenersByDevice.get(deviceId) || []) listener(revision);
}
