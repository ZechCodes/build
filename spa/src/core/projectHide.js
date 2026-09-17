// Putting one project away, on a machine that has gone.
//
// A project belongs to the machine that registered it, so a machine that is away
// leaves its projects standing on the rail with nothing able to move them:
// nothing refreshes them, no verb in them can be carried out, and an account
// that has retired a laptop reads that laptop's blocks every day for work it
// will never pick up. The rail offers such a block one thing — hide — and this
// is what hide does.
//
// HIDE, NOT DELETE. Nothing on the machine is touched and nothing is remembered
// here: a project on a gone machine is nothing but cache, so dropping the cache
// is the whole mechanism. There is deliberately no hidden list — a list would
// mean the project stayed gone after its machine came back, which is a
// disappearance the user never asked for and could not undo. The machine lists
// it again, the block returns, and that is the correct behaviour rather than a
// hole in this one.
//
// Two layers hold it, and both have to let go in the same breath: the snapshot
// the feed is serving out of memory (core/taskFeed.js) and the per-device record
// on disk that seeds the boot paint (core/localCache.js). Clearing only the disk
// would repaint the block from memory on the next tick; clearing only memory
// would bring it back on the next reload.

import { projectEntityIds, withoutProject } from "./feedMerge.js";
import { evictEntity, readCached, writeCached } from "./localCache.js";
import { dropFeedProject } from "./taskFeed.js";

/** Where a device's whole feed snapshot is cached — the same address the sync
 *  layer writes each live answer to (core/cacheSync.js) and the feed seeds its
 *  boot paint from, so a hide rewrites the record they already share. */
const feedAddress = (deviceId) => ({ deviceId, entityId: "", kind: "feed" });

/** Every entity the project holds, across both layers: the snapshot may carry
 *  rows the last persisted record did not (and the other way round after a
 *  reload), and an entity missed here keeps a conversation, a diff and a file
 *  tree cached for a project the reader has put away. */
const entitiesOf = (views, projectKey) =>
  new Set(views.filter(Boolean).flatMap((view) => projectEntityIds(view, projectKey)));

/**
 * Hide one project: its block, its rows, and everything cached under it on the
 * machine it is on.
 *
 * The in-memory drop is synchronous and first, so the block leaves on the press
 * rather than when the database answers. The eviction and the rewritten record
 * follow; a cache that is unavailable (private mode, a browser that refuses
 * IndexedDB) quietly does nothing, which is the contract every reader of
 * core/localCache.js is written against.
 */
export async function hideProject({ deviceId, projectKey }) {
  if (!deviceId || !projectKey) return;
  const live = dropFeedProject(deviceId, projectKey);
  const address = feedAddress(deviceId);
  const cached = (await readCached(address))?.value || null;
  for (const entityId of entitiesOf([live, cached], projectKey)) {
    await evictEntity(deviceId, entityId);
  }
  if (cached) await writeCached(address, withoutProject(cached, projectKey));
}
