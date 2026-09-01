// The cache's sync layer. One tab per browser holds the sync lock and follows
// the feed: every snapshot is persisted for an instant boot paint, entities the
// active set stops naming are evicted on the spot (going Recent, being cleared,
// and finishing are all "stops naming"), and every active branch's git status
// and commit list are kept warm — those are what the Changes surface paints
// first. Thread bodies, diffs, file trees and console tabs are warmed by the
// surfaces themselves on first open (write-through), not here.
//
// Everything here is fire-and-forget against the cache: a failed write is a
// cold revisit, never an error the user sees.

import { App } from "../app.js";
import { subscribeFeed } from "./taskFeed.js";
import { watchChanges } from "./changeEvents.js";
import { cacheableEntityIds } from "./inbox.js";
import { entityIdOf } from "./entityId.js";
import { railEntity } from "./agentRailModel.js";
import { FIRST_PAGE_ITEMS, windowFromThreadPayload } from "./thread.js";
import { cachedEntityIds, cachedSubKeys, evictEntity, writeCached } from "./localCache.js";

const SYNC_LOCK = "build.cacheSync";

/** The safety cadence behind push events, and the poll on a bridge without
 *  them. Background freshness, not liveness — an open surface has its own,
 *  much faster read. */
const ENTITY_REFRESH_MS = 60000;

let unsubscribe = null;
let holdingLock = false;
let releaseLock = null;
const entityWatchers = new Map(); // entityId → { dispose }
const refreshing = new Set(); // entityIds mid-fetch, so deliveries never stack
let activeRows = new Map(); // entityId → its feed row (scope lives on the row)

const deviceIdNow = () => (App.session && App.session.deviceId) || null;

/** The git scope a feed row's checkout answers under — the same derivation the
 *  branch surface makes (views/branchView.js branchScope), minus the primary
 *  case: a primary row names no entity, so it never reaches here. */
function gitScopeOf(row) {
  if (row.kind === "issue") return null;
  if (row.run_id) return { run_id: row.run_id };
  if (row.project_id && row.worktree_id) return { project_id: row.project_id, worktree_id: row.worktree_id };
  return null;
}

/** Re-read the conversations that were ever warmed on this entity — one full
 *  first page per agent, stored as the saved window the rail seeds from. A
 *  conversation never opened has no record here and is never asked for. */
async function refreshThreads(deviceId, entityId, row) {
  const isIssue = row.kind === "issue";
  const detailParams = isIssue ? { issue_id: entityId } : { project_id: row.project_id, branch: row.branch };
  for (const agentSub of await cachedSubKeys(deviceId, entityId, "thread")) {
    try {
      const payload = await App.call(isIssue ? "issue.get" : "branch.get", {
        ...detailParams,
        ...(agentSub ? { agent_id: agentSub } : {}),
        thread_limit: FIRST_PAGE_ITEMS,
      });
      const shaped = windowFromThreadPayload(railEntity(payload, isIssue ? "issue" : "branch").thread);
      if (shaped) await writeCached({ deviceId, entityId, kind: "thread", sub: agentSub }, shaped);
    } catch {
      /* transient, or the agent left — the next event tries again */
    }
  }
}

/** Re-read one active entity into the cache: a branch's status (with the
 *  uncommitted patch emptied — it loads on demand) and commit list, and every
 *  conversation that was ever warmed on it. */
async function refreshEntity(entityId) {
  const deviceId = deviceIdNow();
  const row = activeRows.get(entityId);
  if (!deviceId || !row || refreshing.has(entityId)) return;
  refreshing.add(entityId);
  try {
    const scope = gitScopeOf(row);
    if (scope) {
      try {
        const [status, log] = await Promise.all([App.call("git.status", scope), App.call("git.log", scope)]);
        await writeCached({ deviceId, entityId, kind: "status" }, { ...status, patch: "" });
        await writeCached({ deviceId, entityId, kind: "log" }, log);
      } catch {
        /* offline or mid-switch — the next event or safety poll tries again */
      }
    }
    await refreshThreads(deviceId, entityId, row);
  } finally {
    refreshing.delete(entityId);
  }
}

async function onSnapshot(snapshot) {
  const deviceId = deviceIdNow();
  // The feed's boot paint is this cache talking; only live answers are news.
  if (!deviceId || !holdingLock || snapshot.cached) return;
  await writeCached({ deviceId, entityId: "", kind: "feed" }, snapshot);

  const active = new Set(cacheableEntityIds({ items: snapshot.items }));
  activeRows = new Map();
  for (const item of snapshot.items || []) {
    const id = entityIdOf(item);
    if (id && active.has(id)) activeRows.set(id, item);
  }

  // Immediate eviction: whatever holds records but is no longer named.
  for (const cachedId of await cachedEntityIds(deviceId)) {
    if (!active.has(cachedId)) await evictEntity(deviceId, cachedId);
  }

  // The watcher set follows the active set; a branch entering it syncs now.
  for (const [id, watcher] of entityWatchers) {
    if (active.has(id)) continue;
    watcher.dispose();
    entityWatchers.delete(id);
  }
  for (const id of activeRows.keys()) {
    if (entityWatchers.has(id)) continue;
    entityWatchers.set(id, watchChanges({ refresh: () => refreshEntity(id), intervalMs: ENTITY_REFRESH_MS, entity: id }));
    refreshEntity(id);
  }
}

/** Take the browser-wide sync lock, or queue for it. The holder does the whole
 *  job; every other tab only writes through what its own surfaces read. Without
 *  a Locks API (jsdom, old browsers) this tab just syncs — duplicate fetches
 *  between tabs cost what two open tabs polling always cost. */
function acquireLock() {
  const locks = typeof navigator !== "undefined" ? navigator.locks : undefined;
  if (!locks) {
    holdingLock = true;
    return;
  }
  locks
    .request(SYNC_LOCK, () => {
      holdingLock = true;
      return new Promise((resolve) => {
        releaseLock = resolve;
      });
    })
    .catch(() => {
      /* the lock died with the tab that held it; stopCacheSync resolves ours */
    });
}

export function startCacheSync() {
  stopCacheSync();
  acquireLock();
  unsubscribe = subscribeFeed(onSnapshot);
}

export function stopCacheSync() {
  if (unsubscribe) unsubscribe();
  unsubscribe = null;
  for (const watcher of entityWatchers.values()) watcher.dispose();
  entityWatchers.clear();
  refreshing.clear();
  activeRows = new Map();
  holdingLock = false;
  if (releaseLock) releaseLock();
  releaseLock = null;
}
