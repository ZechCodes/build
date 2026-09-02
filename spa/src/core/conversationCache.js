import { readCached, writeCached } from "./localCache.js";
import { THREAD_RECORD_KIND } from "./thread.js";
import {
  surfacesCacheAddress,
  surfacesFingerprint,
  surfacesFromRecord,
  surfacesRecord,
} from "./surfacesCache.js";

const threadCacheAddress = ({ deviceId, entityId, agentId }) => ({
  deviceId,
  entityId,
  kind: THREAD_RECORD_KIND,
  sub: agentId || "",
});

export function createConversationCache({ addressOf, threadCache, onThreadSeeded, onSurfacesSeeded }) {
  let seedTried = false;
  let persistedSequence = 0;
  let surfacesOnDisk = null;
  let surfacesAnswered = false;

  const seedThreadWindow = (record, seededFor) => {
    if (!record || !threadCache.seedWindow(record.value)) return;
    persistedSequence = record.value.deliveredSequence || 0;
    onThreadSeeded(seededFor);
  };

  const seedSurfaces = (record) => {
    const seen = surfacesFromRecord(record);
    if (!seen || surfacesAnswered) return;
    surfacesOnDisk = surfacesFingerprint(seen.surfaces);
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
      seedSurfaces(surfaces);
      seedThreadWindow(thread, seededFor);
    },

    persistThread() {
      const identity = addressOf();
      const window = threadCache.readWindow();
      if (!identity || !window || window.deliveredSequence === persistedSequence) return;
      persistedSequence = window.deliveredSequence;
      writeCached(threadCacheAddress(identity), window);
    },

    absorbSurfaces(surfaces) {
      surfacesAnswered = true;
      const identity = addressOf();
      const arriving = surfacesFingerprint(surfaces);
      if (!identity || !arriving || arriving === surfacesOnDisk) return;
      surfacesOnDisk = arriving;
      writeCached(surfacesCacheAddress(identity), surfacesRecord(surfaces));
    },

    reset() {
      threadCache.reset();
      seedTried = false;
      persistedSequence = 0;
      surfacesOnDisk = null;
      surfacesAnswered = false;
    },
  };
}
