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
import { FIRST_PAGE_ITEMS, THREAD_RECORD_KIND, windowFromThreadPayload } from "./thread.js";
import { cachedEntityIds, cachedSubKeys, evictEntity, readCached, writeCached } from "./localCache.js";
import { createFileDiffs } from "./fileDiffs.js";
import { surfacesCacheAddress, surfacesFingerprint, surfacesFromRecord, surfacesRecord } from "./surfacesCache.js";
import { coordinatedRead, rpcReadKey } from "./readRequests.js";

const SYNC_LOCK = "build.cacheSync";
const INITIAL_FILE_WARM_BUDGET = 3;

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

const syncContext = () => {
  const session = App.session;
  const scope = App.cacheScope;
  return {
    deviceId: (scope && scope.deviceId) || (session && session.deviceId) || null,
    call: App.call,
    requestScope: scope || session,
    active: () => scope ? scope === App.cacheScope && scope.active() : session === App.session,
  };
};

/** The git scope a feed row's checkout answers under — the same derivation the
 *  branch surface makes (views/branchView.js branchScope), minus the primary
 *  case: a primary row names no entity, so it never reaches here. */
function gitScopeOf(row) {
  if (row.kind === "issue") return null;
  if (row.run_id) return { run_id: row.run_id };
  if (row.project_id && row.worktree_id) return { project_id: row.project_id, worktree_id: row.worktree_id };
  return null;
}

async function writeSurfaces(context, entityId, agents) {
  for (const agent of agents) {
    if (!context.active()) return;
    if (!agent.id || !agent.surfaces) continue;
    const address = surfacesCacheAddress({ deviceId: context.deviceId, entityId, agentId: agent.id });
    const stored = surfacesFromRecord(await readCached(address));
    if (!context.active()) return;
    if (stored && surfacesFingerprint(stored.surfaces) === surfacesFingerprint(agent.surfaces)) continue;
    await writeCached(address, surfacesRecord(agent.surfaces));
  }
}

/** Re-read the conversations that were ever warmed on this entity — one full
 *  first page per agent, stored as the saved window the rail seeds from. A
 *  conversation never opened has no record here and is never asked for. */
async function refreshThreads(context, entityId, row) {
  const isIssue = row.kind === "issue";
  const detailParams = isIssue ? { issue_id: entityId } : { project_id: row.project_id, branch: row.branch };
  let agentsOnEntity = [];
  for (const agentSub of await cachedSubKeys(context.deviceId, entityId, THREAD_RECORD_KIND)) {
    if (!context.active()) return;
    try {
      const payload = await context.call(isIssue ? "issue.get" : "branch.get", {
        ...detailParams,
        ...(agentSub ? { agent_id: agentSub } : {}),
        thread_limit: FIRST_PAGE_ITEMS,
      });
      const detail = railEntity(payload, isIssue ? "issue" : "branch");
      agentsOnEntity = detail.agents;
      const shaped = windowFromThreadPayload(detail.thread);
      if (shaped && context.active()) {
        await writeCached({ deviceId: context.deviceId, entityId, kind: THREAD_RECORD_KIND, sub: agentSub }, shaped);
      }
    } catch {
      /* transient, or the agent left — the next event tries again */
    }
  }
  await writeSurfaces(context, entityId, agentsOnEntity);
}

/** Keep a branch's file listings warm: the top-level directory always — the
 *  Files tab's first paint — plus whichever directories the reader has walked
 *  into, which are the tree records the cache already holds. */
async function refreshTrees(context, entityId, scope) {
  const visited = await cachedSubKeys(context.deviceId, entityId, "tree");
  for (const path of new Set(["", ...visited])) {
    if (!context.active()) return;
    try {
      const listing = await context.call("fs.tree", { ...scope, path });
      if (context.active()) {
        await writeCached(
          { deviceId: context.deviceId, entityId, kind: "tree", sub: path },
          { path: listing.path || "", entries: listing.entries || [] },
        );
      }
    } catch {
      /* transient, or the directory left with a branch switch */
    }
  }
}

/** Keep a warmed review diff fresh — only where the reader has opened the
 *  All-changes view before, which is the record's existence. */
function diffRead(row, held) {
  const runId = row.run_id;
  const method = runId ? "run.diff" : "worktree.diff";
  const repository = runId ? `run:${runId}` : `worktree:${row.project_id}:${row.worktree_id}`;
  const baseParams = runId
    ? { run_id: runId }
    : { project_id: row.project_id, worktree_id: row.worktree_id };
  const params = held?.diff_key ? { ...baseParams, if_diff_key: held.diff_key } : baseParams;
  return { method, repository, params };
}

async function refreshDiff(context, entityId, row) {
  if (row.kind === "issue") return;
  const warmed = await cachedSubKeys(context.deviceId, entityId, "diff");
  if (!warmed.length) return;
  try {
    const address = { deviceId: context.deviceId, entityId, kind: "diff" };
    const cached = await readCached(address);
    const held = cached?.value;
    const { method, repository, params } = diffRead(row, held);
    const diff = await coordinatedRead({
      key: rpcReadKey({
        deviceId: context.deviceId,
        requestScope: context.requestScope,
        repository,
        call: context.call,
        method,
        params,
      }),
      priority: "background",
      load: () => context.call(method, params),
    });
    if (context.active() && !diff.unchanged) {
      await writeCached(
        address,
        { ...held, ...diff, triage: held?.triage || null, projectId: row.project_id || null },
      );
    }
  } catch {
    /* transient — the next event tries again */
  }
}

/** Re-read a checkout's git state: the status shape as received (it carries no
 *  patch — each file's body is its own record) and the commit list. Answers the
 *  shape, so the caller can warm the bodies it names. */
const statusParams = (scope, held) =>
  held?.status_key ? { ...scope, if_status_key: held.status_key } : scope;

async function persistGitState(context, entityId, address, held, answer, log) {
  const status = answer?.unchanged ? held : answer;
  if (!answer?.unchanged) await writeCached(address, status);
  await writeCached({ deviceId: context.deviceId, entityId, kind: "log" }, log);
  return status;
}

async function refreshGitState(context, entityId, scope) {
  try {
    const address = { deviceId: context.deviceId, entityId, kind: "status" };
    const cached = await readCached(address);
    if (!context.active()) return null;
    const held = cached && cached.value;
    const [answer, log] = await Promise.all([context.call("git.status", statusParams(scope, held)), context.call("git.log", scope)]);
    if (!context.active()) return null;
    return persistGitState(context, entityId, address, held, answer, log);
  } catch {
    /* offline or mid-switch — the next event or safety poll tries again */
    return null;
  }
}

/** Warm the bodies of the files a status names — one bounded git.diff per pass,
 *  asking only for what the cache does not already hold — so opening the
 *  Changes surface expands a file with no round trip, offline included. */
async function warmFileDiffs(context, entityId, scope, status) {
  const diffs = createFileDiffs({
    deviceId: context.deviceId,
    entityId,
    scope,
    call: context.call,
    requestPriority: "background",
    requestScope: context.requestScope,
  });
  try {
    if (context.active()) await diffs.warm(status, { budget: INITIAL_FILE_WARM_BUDGET });
  } catch {
    /* transient — the next event or safety poll warms it again */
  } finally {
    diffs.dispose();
  }
}

/** Re-read one active entity into the cache: a branch's git state and the
 *  bodies it names, and every conversation that was ever warmed on it. The warm
 *  waits for an idle turn, so it runs alongside the refresh rather than inside
 *  it — an entity whose tab never goes idle still syncs on the next tick. */
async function refreshEntity(entityId) {
  const context = syncContext();
  const row = activeRows.get(entityId);
  if (!context.deviceId || !row || refreshing.has(entityId)) return;
  refreshing.add(entityId);
  try {
    const scope = gitScopeOf(row);
    if (scope) {
      const status = await refreshGitState(context, entityId, scope);
      await refreshTrees(context, entityId, scope);
      await refreshDiff(context, entityId, row);
      if (status && context.active()) void warmFileDiffs(context, entityId, scope, status);
    }
    await refreshThreads(context, entityId, row);
  } finally {
    refreshing.delete(entityId);
  }
}

// eslint-disable-next-line complexity -- ratchet: onSnapshot is at 14, cap 10 — reduce it, then drop this line
async function onSnapshot(snapshot) {
  const context = syncContext();
  // The feed's boot paint is this cache talking; only live answers are news.
  if (!context.deviceId || !holdingLock || snapshot.cached) return;
  await writeCached({ deviceId: context.deviceId, entityId: "", kind: "feed" }, snapshot);
  if (!context.active()) return;

  const active = new Set(cacheableEntityIds({ items: snapshot.items }));
  activeRows = new Map();
  for (const item of snapshot.items || []) {
    const id = entityIdOf(item);
    if (id && active.has(id)) activeRows.set(id, item);
  }

  // Immediate eviction: whatever holds records but is no longer named.
  for (const cachedId of await cachedEntityIds(context.deviceId)) {
    if (!context.active()) return;
    if (!active.has(cachedId)) await evictEntity(context.deviceId, cachedId);
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
