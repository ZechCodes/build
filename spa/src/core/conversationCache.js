// One agent's conversation, as the local cache holds it: the thread window its
// history is seeded from and the surfaces snapshot its pills stand up from,
// read together on the way in and written back through on the way out.
//
// Nothing here paints. What a seed means on screen is the rail's answer, given
// back through onThreadSeeded / onSurfacesSeeded — so this module runs without
// a document, and the rail keeps no record shapes of its own.

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

/**
 * `addressOf()` says whose records these are — `{ deviceId, entityId, agentId }`
 * — or null while the entity is not yet known or no session device is live.
 * `threadCache` is the window a seed fills and a persist reads.
 *
 * The seed reports through `onThreadSeeded(agentId)` and
 * `onSurfacesSeeded({ surfaces, at })`, and only for the agent the read was
 * made for: a reader who opened another bubble mid-read is told nothing.
 */
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
    /** Both saved records, once per conversation, dropped whole if the reader
     *  opened another bubble while the read was in flight. */
    async seed() {
      const identity = addressOf();
      if (!identity || seedTried) return;
      seedTried = true;
      const seededFor = identity.agentId;
      const [thread, surfaces] = await Promise.all([
        readCached(threadCacheAddress(identity)),
        readCached(surfacesCacheAddress(identity)),
      ]);
      const standing = addressOf();
      if (!standing || standing.agentId !== seededFor) return;
      seedSurfaces(surfaces);
      seedThreadWindow(thread, seededFor);
    },

    /** Write the window through when it has moved. Fire-and-forget,
     *  sequence-guarded: a repaint that absorbed nothing new writes nothing. */
    persistThread() {
      const identity = addressOf();
      const window = threadCache.readWindow();
      if (!identity || !window || window.deliveredSequence === persistedSequence) return;
      persistedSequence = window.deliveredSequence;
      writeCached(threadCacheAddress(identity), window);
    },

    /** A payload has answered about this agent's surfaces, so the seed's turn
     *  is over whatever the answer says. A snapshot that moved is written
     *  through for the next visit; one that stands still, or an answer with
     *  nothing to say, leaves the record where it is. */
    absorbSurfaces(surfaces) {
      surfacesAnswered = true;
      const identity = addressOf();
      const arriving = surfacesFingerprint(surfaces);
      if (!identity || !arriving || arriving === surfacesOnDisk) return;
      surfacesOnDisk = arriving;
      writeCached(surfacesCacheAddress(identity), surfacesRecord(surfaces));
    },

    /** Let this conversation go — with it the seed's one-shot flag, so the
     *  next one seeds from its own records. */
    reset() {
      threadCache.reset();
      seedTried = false;
      persistedSequence = 0;
      surfacesOnDisk = null;
      surfacesAnswered = false;
    },
  };
}
