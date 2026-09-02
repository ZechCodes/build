// The one spelling of an agent's surfaces record: its kind, its address, its
// shape, and what a read of it means. Two writers keep it — the rail, from the
// payload it paints, and the background sync, from the detail read it already
// makes — so neither writes the kind string or the record literal itself and
// they cannot drift apart.
//
// The stamp belongs to the cache (localCache wraps every record as
// `{ at, value }`), so nothing here dates its own writes: how old a snapshot is
// is what decides whether its lingering kinds are still worth painting.

export const SURFACES_RECORD_KIND = "surfaces";

export const surfacesCacheAddress = ({ deviceId, entityId, agentId }) => ({
  deviceId,
  entityId,
  kind: SURFACES_RECORD_KIND,
  sub: agentId || "",
});

export const surfacesRecord = (surfaces) => ({ surfaces });

/** What a stored record holds: the snapshot and when it was written, or null
 *  for a record that was never written, or holds anything but a snapshot. */
export function surfacesFromRecord(record) {
  const surfaces = record && record.value ? record.value.surfaces : null;
  if (!surfaces || typeof surfaces !== "object" || Array.isArray(surfaces)) return null;
  return { surfaces, at: Number.isFinite(record.at) ? record.at : 0 };
}

/** What "the snapshot moved" means to every writer: a value to compare, never
 *  one to store. */
export const surfacesFingerprint = (surfaces) => (surfaces ? JSON.stringify(surfaces) : null);
