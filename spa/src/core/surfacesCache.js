export const SURFACES_RECORD_KIND = "surfaces";

export const surfacesCacheAddress = ({ deviceId, entityId, agentId }) => ({
  deviceId,
  entityId,
  kind: SURFACES_RECORD_KIND,
  sub: agentId || "",
});

export const surfacesRecord = (surfaces) => ({ surfaces });

export function surfacesFromRecord(record) {
  const surfaces = record && record.value ? record.value.surfaces : null;
  if (!surfaces || typeof surfaces !== "object" || Array.isArray(surfaces)) return null;
  return { surfaces, at: Number.isFinite(record.at) ? record.at : 0 };
}

export const surfacesFingerprint = (surfaces) => (surfaces ? JSON.stringify(surfaces) : null);
