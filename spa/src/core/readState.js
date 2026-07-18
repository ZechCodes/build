// Persistent read-state for notification-bearing entities (runs + plans).
// Pure functions over an injected Web-Storage-shaped object ({getItem, setItem})
// so the module is unit-testable under node; the app passes localStorage.
// The set is pruned against the live feed every tick so it cannot grow forever,
// and an id that leaves and re-enters needs-attention stays read as long as the
// entity itself still exists.

export const READ_IDS_KEY = "build.readIds.v1";

/** The persisted read ids as a Set. Missing key, corrupt JSON, or a non-array
 *  payload all yield an empty Set — read-state is a convenience, never fatal. */
export function loadReadIds(storage) {
  try {
    const parsed = JSON.parse(storage.getItem(READ_IDS_KEY));
    return Array.isArray(parsed) ? new Set(parsed) : new Set();
  } catch {
    return new Set();
  }
}

export function persistReadIds(readIds, storage) {
  storage.setItem(READ_IDS_KEY, JSON.stringify([...readIds]));
}

/** A NEW Set keeping only the read ids still present in liveIds (a Set or
 *  array of every current run_id + plan_id — all of them, not just the
 *  needs-attention ones). Pure: the input set is never mutated. */
export function pruneReadIds(readIds, liveIds) {
  const live = liveIds instanceof Set ? liveIds : new Set(liveIds);
  return new Set([...readIds].filter((id) => live.has(id)));
}
