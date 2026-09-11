// One shared board.list-feed poller for surfaces that live on every page (the
// sidebar, the nav badge) — views keep their own detail polls. The plan/run
// split means the feed carries both collections; consumers read `plans` and
// `runs` separately.

import { App } from "../app.js";
import { watchChanges } from "./changeEvents.js";
import { readCached } from "./localCache.js";

const subscribers = new Set();
let watcher = null;
let last = null;

const EMPTY_SCOPED_FEED = Object.freeze({
  items: [],
  plans: [],
  runs: [],
  externalWorktrees: [],
  pending: [],
  primaryChanges: [],
  projects: [],
  workspaces: [],
  cached: true,
});

/** Subscribe to feed snapshots ({items, plans, runs, externalWorktrees,
 *  pending, projects, primaryChanges}); the current snapshot (if any) is
 *  delivered immediately. Returns unsubscribe. */
export function subscribeFeed(fn) {
  subscribers.add(fn);
  if (last) fn(last);
  return () => subscribers.delete(fn);
}

/** Retire the snapshot owned by the device being left. The cached marker keeps
 * the sync layer from treating this boundary value as bridge data to persist. */
export function resetFeedScope() {
  last = EMPTY_SCOPED_FEED;
  subscribers.forEach((fn) => fn(last));
}

/** The run that owns a project's primary checkout, or null while nobody has
 *  adopted it. The bridge stamps the owner onto the feed's primary-changes
 *  entry, which makes this a READ: a surface can bind to an existing owner
 *  without minting one. A terminal run has let go, so it is reported as null. */
export function primaryRunIdFor(feed, projectId) {
  const entry = ((feed && feed.primaryChanges) || []).find((e) => e.project_id === projectId);
  return (entry && entry.run_id) || null;
}

const ownsFeedContext = ({ session, scope }) =>
  session === App.session && scope === App.cacheScope && (!scope || scope.active());

const liveFeedSnapshot = (board, projectList, workspaceList) => ({
  // The redesigned feed: one row per work item (branch or issue). The
  // legacy collections below still ship, and still feed what has not moved
  // over yet.
  items: board.items || [],
  plans: board.plans || [],
  runs: board.runs || [],
  externalWorktrees: board.external_worktrees || [],
  // The lifecycle verbs whose git is running right now: a checkout being
  // cut is a row from the moment it is asked for, under the id it will
  // settle as. A bridge that predates them sends none.
  pending: board.pending || [],
  primaryChanges: board.primary_changes || [],
  // The wire names a project by `project_id`; consumers of the snapshot
  // (the toolbar's scope and menu) read `id`. Bridge the key here, in the
  // one place the wire is read.
  projects: (projectList.projects || []).map((project) => ({
    ...project,
    id: project.project_id || project.id,
  })),
  workspaces: workspaceList?.workspaces || [],
});

async function tick() {
  const context = { session: App.session, scope: App.cacheScope };
  const call = App.call;
  try {
    const [board, projectList, workspaceList] = await Promise.all([
      call("board.list"),
      call("project.list"),
      // A workspace-list failure must not take the board and agent rails down
      // with it; the next feed tick will retry the landing list independently.
      Promise.resolve(call("workspace.list")).catch(() => ({ workspaces: [] })),
    ]);
    if (!ownsFeedContext(context)) return;
    last = liveFeedSnapshot(board, projectList, workspaceList);
    subscribers.forEach((fn) => fn(last));
  } catch {
    /* offline / transient — the next tick retries */
  }
}

// Refocusing a tab refreshes immediately instead of waiting out the interval —
// the visible counterpart of hidden tabs skipping their ticks.
const onVisibilityChange = () => {
  if (!document.hidden) tick();
};

/** The last snapshot the syncer persisted, painted while the bridge is still
 *  being asked. Marked `cached: true` so the sync layer does not treat its own
 *  echo as news; a live answer that gets there first wins outright. */
async function seedFromCache() {
  const session = App.session;
  const scope = App.cacheScope;
  const deviceId = (session && session.deviceId) || App.selectedDeviceId;
  if (!deviceId) return;
  const record = await readCached({ deviceId, entityId: "", kind: "feed" });
  if (!record || last || session !== App.session || scope !== App.cacheScope || (scope && !scope.active())) return;
  // Nothing in flight survives a reload: the verbs the last session watched
  // settled long ago, and the live answer names whatever is running now.
  last = { ...record.value, pending: [], cached: true };
  subscribers.forEach((fn) => fn(last));
}

export function startFeed(intervalMs = 2000) {
  stopFeed();
  seedFromCache();
  tick();
  // The feed is the board, so `board.changed` is its event and this interval is
  // the safety poll behind it. It owns its own visible-again catch-up above —
  // which reads whether or not anything was pushed — so the registry leaves
  // that alone rather than reading twice.
  watcher = watchChanges({ refresh: tick, intervalMs, catchUpOnVisible: false });
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", onVisibilityChange);
  }
}

export function stopFeed() {
  if (watcher) watcher.dispose();
  watcher = null;
  if (typeof document !== "undefined") {
    document.removeEventListener("visibilitychange", onVisibilityChange);
  }
}

/** Force an immediate refresh (after adding a project, adopting, …). */
export function refreshFeed() {
  return tick();
}
