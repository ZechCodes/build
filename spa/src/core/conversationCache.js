import { readCached, writeCached } from "./localCache.js";
import { THREAD_RECORD_KIND } from "./thread.js";
import {
  surfacesCacheAddress,
  surfacesFingerprint,
  surfacesFromRecord,
  surfacesRecord,
  surfaceSessionGeneration,
} from "./surfacesCache.js";

const threadCacheAddress = ({ deviceId, entityId, agentId, conversationId }) => ({
  deviceId,
  // The workspace is the entity a transcript is stored under, always: that
  // prefix is what Done, Delete and the 72 h expiry sweep, and a record
  // addressed outside it would outlive the workspace it belongs to for ever.
  entityId,
  kind: THREAD_RECORD_KIND,
  // A conversation is the transcript's canonical storage owner within the
  // workspace. Issue and run views may intentionally point at the same one;
  // their agent ids must not fork that history into two browser caches.
  sub: conversationId || agentId || "",
});

export function createConversationCache({ addressOf, threadCache, onThreadSeeded, onSurfacesSeeded }) {
  let seedTried = false;
  let persistedSequence = 0;
  let surfacesOnDisk = null;
  let surfacesAnswered = false;
  let surfaceWriteRevision = 0;

  const seedThreadWindow = (record, seededFor) => {
    if (!record || !threadCache.seedWindow(record.value)) return;
    persistedSequence = record.value.deliveredSequence || 0;
    onThreadSeeded(seededFor);
  };

  const sameSurfaceIdentity = (captured, current) =>
    current
    && current.deviceId === captured.deviceId
    && current.entityId === captured.entityId
    && current.agentId === captured.agentId
    && surfaceSessionGeneration(current.surfaceSessionGeneration)
      === surfaceSessionGeneration(captured.surfaceSessionGeneration);

  const seedSurfaces = (record, identity) => {
    const generation = surfaceSessionGeneration(identity.surfaceSessionGeneration);
    const seen = surfacesFromRecord(record, generation);
    if (!seen || surfacesAnswered) return;
    surfacesOnDisk = surfacesFingerprint(seen.surfaces, generation);
    onSurfacesSeeded(seen);
  };

  return {
    async seed() {
      const identity = addressOf();
      if (!identity || seedTried) return;
      seedTried = true;
      const seededFor = identity.agentId;
      const [thread, surfaces] = await Promise.all([
        readCached(threadCacheAddress(identity)),
        readCached(surfacesCacheAddress(identity)),
      ]);
      const stillOpen = addressOf();
      if (!stillOpen || stillOpen.agentId !== seededFor) return;
      if (sameSurfaceIdentity(identity, stillOpen)) seedSurfaces(surfaces, identity);
      seedThreadWindow(thread, seededFor);
    },

    persistThread() {
      const identity = addressOf();
      const window = threadCache.readWindow();
      if (!identity || !window || window.deliveredSequence === persistedSequence) return;
      persistedSequence = window.deliveredSequence;
      writeCached(threadCacheAddress(identity), window);
    },

    absorbSurfaces(surfaces, generationValue) {
      surfacesAnswered = true;
      const writeRevision = ++surfaceWriteRevision;
      const identity = addressOf();
      const generation = surfaceSessionGeneration(generationValue);
      const arriving = surfacesFingerprint(surfaces, generation);
      if (!identity || generation !== surfaceSessionGeneration(identity.surfaceSessionGeneration) || !arriving) return;
      void (async () => {
        const saved = surfacesFromRecord(
          await readCached(surfacesCacheAddress(identity)),
          generation,
          false,
        );
        const current = addressOf();
        if (writeRevision !== surfaceWriteRevision || !sameSurfaceIdentity(identity, current)) return;
        const savedFingerprint = saved && surfacesFingerprint(saved.surfaces, saved.generation);
        if (arriving === surfacesOnDisk || arriving === savedFingerprint) {
          surfacesOnDisk = arriving;
          return;
        }
        surfacesOnDisk = arriving;
        await writeCached(surfacesCacheAddress(identity), surfacesRecord(surfaces, generation));
      })();
    },

    reset() {
      threadCache.reset();
      seedTried = false;
      persistedSequence = 0;
      surfacesOnDisk = null;
      surfacesAnswered = false;
      surfaceWriteRevision += 1;
    },
  };
}
