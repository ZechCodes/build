// Metadata invalidations are refresh hints. Consumers reread authoritative
// metadata after reconnect and fence reads by scope/generation/revision.
const listenersByDevice = new Map();
const identifier = (value) => typeof value === "string" && value.length > 0 && value.length <= 256;

/** A per-device reader can refresh project grants without holding their origin
 * conversation. The returned disposer retires its exact listener. */
export function onSecretsChanged(deviceId, listener) {
  const listeners = listenersByDevice.get(deviceId) || new Set();
  listeners.add(listener);
  listenersByDevice.set(deviceId, listeners);
  return () => {
    listeners.delete(listener);
    if (!listeners.size) listenersByDevice.delete(deviceId);
  };
}

function scopeOf(event) {
  if (event.scope === "device" && event.truncated === true && event.secret_ids.length === 0) return { scope: "device" };
  if (event.scope === "environment" && identifier(event.environment_id)) return { scope: "environment", environment_id: event.environment_id };
  if (event.scope === "project" && identifier(event.project_id)) return { scope: "project", project_id: event.project_id };
  if (event.scope !== "agent") return null;
  const keys = ["entity_id", "conversation_id", "thread_id", "generation"];
  if (!keys.every((key) => identifier(event[key]))) return null;
  if (event.thread_id !== event.generation) return null;
  return { scope: "agent", ...Object.fromEntries(keys.map((key) => [key, event[key]])) };
}

function validHint(event) {
  if (!event || !Number.isSafeInteger(event.revision) || event.revision < 1) return false;
  const ids = event.secret_ids;
  if (!Array.isArray(ids) || ids.length > 100 || !ids.every(identifier)) return false;
  return ids.length > 0 || event.truncated === true;
}

/** Project onto the public metadata DTO before any cache reader hears a hint.
 * Names, values and upload envelopes are never forwarded by this seam. */
export function secretsChangedOn(deviceId, event) {
  if (!validHint(event)) return;
  const scope = scopeOf(event);
  if (!scope) return;
  const hint = Object.freeze({ ...scope, revision: event.revision, secret_ids: Object.freeze([...new Set(event.secret_ids)]),
    ...(event.truncated === true ? { truncated: true } : {}) });
  for (const listener of listenersByDevice.get(deviceId) || []) listener(hint);
}
