export const SURFACES_RECORD_KIND = "surfaces";

export const surfacesCacheAddress = ({ deviceId, entityId, agentId }) => ({
  deviceId,
  entityId,
  kind: SURFACES_RECORD_KIND,
  sub: agentId || "",
});

export const surfaceSessionGeneration = (value) =>
  typeof value === "string" && value.length ? value : null;

export const surfacesRecord = (surfaces, generation) => ({
  surfaces,
  generation: surfaceSessionGeneration(generation),
});

const observationAsStale = (observation, hasValue) => {
  if (!observation || typeof observation !== "object" || Array.isArray(observation)) {
    return hasValue ? { support: "supported", freshness: "stale" } : observation;
  }
  if (observation.support === "unsupported") return { ...observation };
  return { ...observation, freshness: "stale" };
};

/** Cached provider observations are useful for instant paint, but cannot claim
 * current freshness before this visit has reconciled them with the process. */
export function staleObservedSurfaces(surfaces) {
  const restored = structuredClone(surfaces);
  const observations = restored.observations && typeof restored.observations === "object"
    ? { ...restored.observations }
    : {};
  if (restored.goal !== undefined || observations.goal) {
    observations.goal = observationAsStale(observations.goal, restored.goal !== undefined);
  }
  if (restored.checklist !== undefined || observations.checklist) {
    observations.checklist = observationAsStale(observations.checklist, restored.checklist !== undefined);
  }
  if (Object.keys(observations).length) restored.observations = observations;
  return restored;
}

const objectSnapshot = (value) =>
  value && typeof value === "object" && !Array.isArray(value) ? value : null;

const savedSnapshot = (value) => {
  if (value === null) return { found: true, surfaces: null };
  const surfaces = objectSnapshot(value);
  return { found: !!surfaces, surfaces };
};

const restoredSnapshot = (surfaces, markObservationsStale) =>
  surfaces && markObservationsStale ? staleObservedSurfaces(surfaces) : surfaces;

const recordTime = (record) => Number.isFinite(record?.at) ? record.at : 0;

export function surfacesFromRecord(record, expectedGeneration, markObservationsStale = true) {
  const expected = surfaceSessionGeneration(expectedGeneration);
  const generation = surfaceSessionGeneration(record?.value?.generation);
  const saved = savedSnapshot(record?.value?.surfaces);
  if (!expected || generation !== expected) return null;
  if (!saved.found) return null;
  return {
    surfaces: restoredSnapshot(saved.surfaces, markObservationsStale),
    generation,
    at: recordTime(record),
  };
}

export const surfacesFingerprint = (surfaces, generation) => {
  const normalized = surfaceSessionGeneration(generation);
  return normalized ? JSON.stringify({ generation: normalized, surfaces }) : null;
};
