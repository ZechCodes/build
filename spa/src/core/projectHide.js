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
// the feed is serving out of memory (core/taskFeed.js) and the per-device
// records on disk that seed the boot paint (core/localCache.js). Clearing only
// the disk would repaint the block from memory on the next tick; clearing only
// memory would bring it back on the next reload.

import { projectEntityIds, rowsWithoutProject, withoutProject } from "./feedMerge.js";
import { evictEntity, mergeCachedAtomically, readCachedMany, writeCached } from "./localCache.js";
import { dropFeedProject } from "./taskFeed.js";

/** The three records a device's board is cached in — the same addresses the
 *  sync layer writes each live answer to (core/cacheSync.js) and the feed
 *  seeds its boot paint from, so a hide rewrites the records they already
 *  share.
 *
 *  All three, because the boot paint reads the two lists from their own
 *  records rather than out of the feed: a board push rewrites those two and
 *  never the feed. A hide that rewrote the feed alone would be undone by the
 *  next reload, and nothing would ever rewrite the lists — hide is only
 *  offered for a machine that has gone, so no live pass is coming. */
const RECORDS = Object.freeze(["feed", "projects", "workspaces"]);

const recordAddress = (deviceId, kind) => ({ deviceId, entityId: "", kind });

/** The record with the project taken out of it: the feed record holds a whole
 *  snapshot and is pruned collection by collection, the other two hold one
 *  collection each and are pruned as they stand. */
const withoutProjectIn = (kind, value, projectKey) =>
  kind === "feed" ? withoutProject(value, projectKey) : rowsWithoutProject(value, projectKey);

/** The three records as views `projectEntityIds` can read: a bare list is the
 *  collection it is named by, so a workspace that rode in on a board push —
 *  and so is named in the workspace record and nowhere else — is evicted with
 *  the rest. */
const viewOf = (kind, value) => (kind === "feed" ? value : { [kind]: value });

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
 * rather than when the database answers. The rewritten records and the eviction
 * follow; a cache that is unavailable (private mode, a browser that refuses
 * IndexedDB) quietly does nothing, which is the contract every reader of
 * core/localCache.js is written against.
 *
 * `evictEntity` rather than `evictWorkspaceData`: the rail reads a row's own
 * record as well as the board list, so a hide that kept the rows would paint
 * the block again on the next announcement. Hide is the one case where the row
 * goes with the data — the machine is gone, and the board that lists it is
 * never going to answer again.
 */
export async function hideProject({ deviceId, projectKey }) {
  if (!deviceId || !projectKey) return;
  const live = dropFeedProject(deviceId, projectKey);
  const records = await readCachedMany(RECORDS.map((kind) => recordAddress(deviceId, kind)));
  const views = [live, ...RECORDS.map((kind, index) => (records[index] ? viewOf(kind, records[index].value) : null))];
  // The three records first, then the entities under them. Every write here is
  // announced, and the rail re-reads on the announcement: evicting first would
  // send it to a board that still names the block, and the project would paint
  // again between the two halves of its own removal.
  for (const [index, kind] of RECORDS.entries()) {
    const held = records[index];
    if (!held) continue;
    const address = recordAddress(deviceId, kind);
    if (kind === "feed") await writeCached(address, withoutProjectIn(kind, held.value, projectKey));
    else await mergeCachedAtomically(address, (current) => current && withoutProjectIn(kind, current, projectKey));
  }
  for (const entityId of entitiesOf(views, projectKey)) {
    await evictEntity(deviceId, entityId);
  }
}
