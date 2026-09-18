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
//
// # Two tiers behind one job (wire spec step 1.6)
//
// Against a bridge that serves subscriptions this layer stops looping on that
// device. It is the background tier's `onChanges` handler: two all-scope
// subscriptions per device — state, thread and git every 30 s, files every 3
// minutes, both background — say which entity moved and to which key, and a
// pull happens only where the pushed key disagrees with the record held. Behind
// them sits one full pass every ten minutes and one when the tab comes back,
// which is the whole of the safety net. A device whose bridge serves no
// subscriptions is unchanged: the per-entity 60 s watcher is exactly the loop it
// has always been, and one device on each contract is ordinary.

import { contextFor } from "./deviceContexts.js";
import { subscribeFeed } from "./taskFeed.js";
import { onSubscriptionsChange, subscriptionsActive, watchChanges } from "./changeEvents.js";
import { cacheableEntityIds } from "./inbox.js";
import { entityIdOf } from "./entityId.js";
import { railEntity } from "./agentRailModel.js";
import { FIRST_PAGE_ITEMS, THREAD_RECORD_KIND, windowFromThreadPayload } from "./thread.js";
import { cachedEntityIds, cachedSubKeys, evictEntity, readCached, writeCached } from "./localCache.js";
import { createFileDiffs } from "./fileDiffs.js";
import {
  surfaceSessionGeneration,
  surfacesCacheAddress,
  surfacesFingerprint,
  surfacesFromRecord,
  surfacesRecord,
} from "./surfacesCache.js";
import { coordinatedRead, requestPriorityFields, rpcReadKey } from "./readRequests.js";
import { pageVisible } from "./visibility.js";

const SYNC_LOCK = "build.cacheSync";
const INITIAL_FILE_WARM_BUDGET = 3;

/** The safety cadence behind push events, and the poll on a bridge without
 *  them. Background freshness, not liveness — an open surface has its own,
 *  much faster read. */
const ENTITY_REFRESH_MS = 60000;

/** The background tier's two cadences, and the safety pass behind them. The
 *  bridge clamps a batch to [1000, 600000]; these sit inside it. */
const STATE_BATCH_MS = 30000;
const FILES_BATCH_MS = 180000;
const BACKGROUND_SWEEP_MS = 600000;

/** Every read this layer makes is a warm-up, so every one of them rides the
 *  wire behind the focused surface's. */
const BACKGROUND = requestPriorityFields("background");

let unsubscribe = null;
let stopModeWatch = null;
let visibilityWired = false;
let holdingLock = false;
let releaseLock = null;
const backgroundWatchers = new Map(); // deviceId → its two all-scope watchers
const entityWatchers = new Map(); // row key → { dispose }
const refreshing = new Set(); // row keys mid-fetch, so deliveries never stack
const warmed = new Set(); // row keys this active set has already read once
let activeRows = new Map(); // row key → { deviceId, entityId, row }

/** An entity belongs to the device it is on: two machines can hold the same id,
 *  and neither one's records are the other's. The pair is written as a key in
 *  this one place. */
const rowKey = (deviceId, entityId) => `${deviceId}|${entityId}`;

/** What the syncer needs of a device, from that device's context. */
const syncContext = (context) =>
  context && {
    deviceId: context.deviceId,
    call: context.rpc,
    requestScope: context.cacheScope,
    active: () => context.active(),
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
    await writeAgentSurfaces(context, entityId, agent);
  }
}

async function writeAgentSurfaces(context, entityId, agent) {
  const agentId = agent.id;
  const generation = surfaceSessionGeneration(agent.surface_session_generation);
  if (!agentId || !generation) return;
  const address = surfacesCacheAddress({ deviceId: context.deviceId, entityId, agentId });
  const stored = surfacesFromRecord(await readCached(address), generation, false);
  if (!context.active()) return;
  if (agent.id !== agentId || surfaceSessionGeneration(agent.surface_session_generation) !== generation) return;
  const arriving = surfacesFingerprint(agent.surfaces ?? null, generation);
  if (stored && surfacesFingerprint(stored.surfaces, stored.generation) === arriving) return;
  await writeCached(address, surfacesRecord(agent.surfaces ?? null, generation));
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
      const payload = await context.call(
        isIssue ? "issue.get" : "branch.get",
        { ...detailParams, ...(agentSub ? { agent_id: agentSub } : {}), thread_limit: FIRST_PAGE_ITEMS },
        BACKGROUND,
      );
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
  await listTrees(context, entityId, scope, new Set(["", ...visited]));
}

/** Re-list exactly these directories. The top level is always among them: it is
 *  the Files tab's first paint. */
async function listTrees(context, entityId, scope, paths) {
  for (const path of paths) {
    if (!context.active()) return;
    try {
      const listing = await context.call("fs.tree", { ...scope, path }, BACKGROUND);
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
  const warmedDiffs = await cachedSubKeys(context.deviceId, entityId, "diff");
  if (!warmedDiffs.length) return;
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
      load: (envelope) => context.call(method, params, envelope),
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
    const [answer, log] = await Promise.all([
      context.call("git.status", statusParams(scope, held), BACKGROUND),
      context.call("git.log", scope, BACKGROUND),
    ]);
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

/** A branch's git state and the bodies it names: status, trees, diff, and the
 *  first files of it. The file warm waits for an idle turn, so it runs
 *  alongside the refresh rather than inside it — an entity whose tab never goes
 *  idle still syncs on the next tick. */
async function refreshGitSurfaces(context, entityId, row) {
  const scope = gitScopeOf(row);
  if (!scope) return; // an issue has no checkout to read
  const status = await refreshGitState(context, entityId, scope);
  await refreshTrees(context, entityId, scope);
  await refreshDiff(context, entityId, row);
  if (status && context.active()) void warmFileDiffs(context, entityId, scope, status);
}

/** Re-read one active entity into the cache: its git surfaces, and every
 *  conversation that was ever warmed on it. */
async function refreshEntity(key) {
  const active = activeRows.get(key);
  const context = active && syncContext(contextFor(active.deviceId));
  if (!context || refreshing.has(key)) return;
  refreshing.add(key);
  const { entityId, row } = active;
  try {
    await refreshGitSurfaces(context, entityId, row);
    await refreshThreads(context, entityId, row);
  } finally {
    refreshing.delete(key);
  }
}

/** The one walk over the held rows by device: a device's rows are replaced
 *  wholesale, or leave with it. Another device's rows are never touched.
 *  `namesDevice` is asked about a DEVICE ID, not a row. */
function dropRowsOf(namesDevice) {
  for (const [key, held] of activeRows) {
    if (namesDevice(held.deviceId)) activeRows.delete(key);
  }
}

/** The rows of one device's view worth keeping records for, replacing whatever
 *  that device named last time and leaving every other device's alone. */
function keepActiveRows(deviceId, view, active) {
  dropRowsOf((heldDeviceId) => heldDeviceId === deviceId);
  for (const row of view.items || []) {
    const entityId = entityIdOf(row);
    if (entityId && active.has(entityId)) activeRows.set(rowKey(deviceId, entityId), { deviceId, entityId, row });
  }
}

/** Immediate eviction: whatever this device holds records for but no longer
 *  names. Another device's records are another device's business. */
async function evictUnnamed(context, active) {
  for (const cachedId of await cachedEntityIds(context.deviceId)) {
    if (!context.active()) return;
    if (!active.has(cachedId)) await evictEntity(context.deviceId, cachedId);
  }
}

/** The per-entity watcher set, which exists only on the legacy path: a device
 *  whose bridge serves subscriptions is covered by its own background tier, and
 *  a 60 s poll beside it would be the loop this design removes. Each device
 *  answers for itself, so one device on each contract is ordinary. */
function syncEntityWatchers() {
  for (const [key, watcher] of entityWatchers) {
    if (wantsEntityWatcher(key)) continue;
    watcher.dispose();
    entityWatchers.delete(key);
  }
  for (const [key, { deviceId, entityId }] of activeRows) {
    if (entityWatchers.has(key) || subscriptionsActive(deviceId)) continue;
    entityWatchers.set(
      key,
      watchChanges({ refresh: () => refreshEntity(key), intervalMs: ENTITY_REFRESH_MS, entity: entityId, deviceId }),
    );
  }
}

/** Whether this row still wants a 60 s watcher: it is still active, and its
 *  device's bridge is still not carrying subscriptions. */
function wantsEntityWatcher(key) {
  const held = activeRows.get(key);
  return Boolean(held) && !subscriptionsActive(held.deviceId);
}

/** An entity entering the active set is read once, whichever contract its device
 *  is on: a subscription only says what moved after it was taken out. */
function warmNewcomers() {
  for (const key of activeRows.keys()) {
    if (warmed.has(key)) continue;
    warmed.add(key);
    void refreshEntity(key);
  }
}

/** A row that left the active set has no read owed to it. */
function forgetLeftRows() {
  for (const key of [...warmed]) {
    if (!activeRows.has(key)) warmed.delete(key);
  }
}

/** Follow one device's view: persist it for that device's boot paint, evict
 *  what it stopped naming, keep what it names warm, and put that device's
 *  background tier up the first time it is seen. */
async function syncDeviceSnapshot(deviceId, view) {
  const context = syncContext(contextFor(deviceId));
  // The feed's boot paint is this cache talking; only live answers are news.
  if (!context || view.cached) return;
  watchBackground(deviceId);
  await writeCached({ deviceId, entityId: "", kind: "feed" }, view);
  if (!context.active()) return;
  const active = new Set(cacheableEntityIds({ items: view.items }));
  keepActiveRows(deviceId, view, active);
  forgetLeftRows();
  await evictUnnamed(context, active);
  syncEntityWatchers();
  warmNewcomers();
}

/** A device that left the feed — retired, signed out — stops being synced: the
 *  rows it named go, their watchers with them, and its background tier comes
 *  down. */
function forgetDevicesMissingFrom(devices) {
  dropRowsOf((heldDeviceId) => !(heldDeviceId in devices));
  for (const [deviceId, watchers] of backgroundWatchers) {
    if (deviceId in devices) continue;
    watchers.forEach((watcher) => watcher.dispose());
    backgroundWatchers.delete(deviceId);
  }
  forgetLeftRows();
  syncEntityWatchers();
}

async function onSnapshot(snapshot) {
  if (!holdingLock) return;
  const devices = snapshot.devices || {};
  forgetDevicesMissingFrom(devices);
  for (const [deviceId, view] of Object.entries(devices)) {
    await syncDeviceSnapshot(deviceId, view);
  }
}

// ---------------------------------------------------------------------------
// The background tier: what one flush of the `changes` event does here.
// ---------------------------------------------------------------------------

/** Every directory a changed path sits under, deepest last: `src/app/x.js`
 *  stales the listings of `src` and `src/app`. */
function dirsOf(paths) {
  const dirs = new Set();
  for (const path of paths || []) {
    const parts = String(path).split("/").slice(0, -1);
    for (let depth = 1; depth <= parts.length; depth++) dirs.add(parts.slice(0, depth).join("/"));
  }
  return [...dirs];
}

/** `files`: the listings the reader walked into are stale where a changed path
 *  sits in them, and all of them when the list was truncated — a truncated list
 *  means "refetch the tree", not "these paths". The bodies follow the `git`
 *  item, which the same write always raises. */
async function applyFiles(context, entityId, row, files) {
  const scope = gitScopeOf(row);
  if (!scope) return;
  const held = await cachedSubKeys(context.deviceId, entityId, "tree");
  const stale = files.truncated ? held : dirsOf(files.paths).filter((dir) => held.includes(dir));
  await listTrees(context, entityId, scope, new Set(["", ...stale]));
}

/** `git`: the pushed `status_key` is the same FNV key `git.status` answers
 *  with, so holding it is proof there is nothing to fetch. */
async function applyGit(context, entityId, row, git) {
  const scope = gitScopeOf(row);
  if (!scope) return;
  const cached = await readCached({ deviceId: context.deviceId, entityId, kind: "status" });
  if (git.status_key && cached?.value?.status_key === git.status_key) return;
  const status = await refreshGitState(context, entityId, scope);
  await refreshDiff(context, entityId, row);
  if (status && context.active()) void warmFileDiffs(context, entityId, scope, status);
}

/** Whether any agent's pushed tip is past the window this cache holds. An
 *  agent whose conversation was never warmed has no record and no claim on a
 *  read. */
async function threadBehind(context, entityId, tips) {
  for (const tip of tips || []) {
    const address = { deviceId: context.deviceId, entityId, kind: THREAD_RECORD_KIND, sub: tip.agent_id || "" };
    const record = await readCached(address);
    if (!record) continue;
    if (Number(tip.last_sequence || 0) > Number(record.value?.deliveredSequence || 0)) return true;
  }
  return false;
}

/** The feed row's own state, in the shape the `state` item carries it: the
 *  bridge fills that item from the same board row this cache holds, so the two
 *  are compared field by field with no translation. */
const rowState = (row) => ({
  run: row.state,
  agents: (row.agents || []).length,
  attention: row.unread_reason || "none",
});

/** Whether a pushed `state` says anything the held row does not. An empty
 *  object — an entity the bridge keeps no row for — names no field and stays
 *  what it always was: the bare "refetch". A field this build does not know is
 *  news too; a later minor never goes unread. */
function stateMoved(row, state) {
  const held = rowState(row);
  const fields = Object.keys(state);
  if (!fields.length) return true;
  return fields.some((field) => String(state[field]) !== String(held[field]));
}

/** `state` and `thread` are both answered from what this cache holds: the
 *  pushed row against the feed row, and each pushed tip against the window
 *  stored for that agent. The detail read happens only where one disagrees. */
async function applyDetail(context, entityId, row, item) {
  const moved = item.state ? stateMoved(row, item.state) : false;
  if (!moved && !(await threadBehind(context, entityId, item.thread))) return;
  await refreshThreads(context, entityId, row);
}

async function applyItem(context, item) {
  const entityId = String(item.entity_id || "");
  const key = rowKey(context.deviceId, entityId);
  const held = activeRows.get(key);
  if (!held || refreshing.has(key)) return;
  refreshing.add(key);
  try {
    if (item.files) await applyFiles(context, entityId, held.row, item.files);
    if (item.git) await applyGit(context, entityId, held.row, item.git);
    if (item.state || item.thread) await applyDetail(context, entityId, held.row, item);
  } finally {
    refreshing.delete(key);
  }
}

/** One flush of one device's background tier. Items for entities that device's
 *  active set no longer names are nothing to this cache: their records left with
 *  them. */
async function applyChanges(items, deviceId) {
  const context = syncContext(contextFor(deviceId));
  if (!context || !holdingLock) return;
  for (const item of items) {
    if (!context.active()) return;
    await applyItem(context, item);
  }
}

/** The safety pass for one device: every active row of its, whatever its bridge
 *  did or did not say. Ten minutes, and once when the tab comes back — only
 *  where that device's subscriptions are carrying, because the legacy path
 *  already has its 60 s loop and must not read twice. */
function sweepDevice(deviceId) {
  if (!subscriptionsActive(deviceId)) return;
  for (const [key, held] of activeRows) {
    if (held.deviceId === deviceId) void refreshEntity(key);
  }
}

/** The same, for every device the tier is up on. */
const sweep = () => [...backgroundWatchers.keys()].forEach(sweepDevice);

function onVisibilityChange() {
  if (pageVisible()) sweep();
}

/** One device's background tier: its whole board, at the two cadences the spec
 *  names, both behind the foreground. The files subscription shares the state
 *  one's sweep rather than running a second. Idempotent — a device already
 *  watched is left as it is. */
function watchBackground(deviceId) {
  if (backgroundWatchers.has(deviceId)) return;
  const onChanges = (items) => void applyChanges(items, deviceId);
  const shared = {
    scope: "all",
    deviceId,
    priority: "background",
    onChanges,
    catchUpOnVisible: false,
    intervalMs: BACKGROUND_SWEEP_MS,
  };
  backgroundWatchers.set(deviceId, [
    watchChanges({ ...shared, refresh: () => sweepDevice(deviceId), kinds: ["state", "thread", "git"], mode: { batch_ms: STATE_BATCH_MS } }),
    watchChanges({ ...shared, refresh: () => {}, kinds: ["files"], mode: { batch_ms: FILES_BATCH_MS } }),
  ]);
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
  // A device reconnecting onto an older bridge takes its subscriptions away; its
  // 60 s per-entity polls come back with it, and vice versa.
  stopModeWatch = onSubscriptionsChange(syncEntityWatchers);
  if (typeof document !== "undefined" && !visibilityWired) {
    document.addEventListener("visibilitychange", onVisibilityChange);
    visibilityWired = true;
  }
}

export function stopCacheSync() {
  if (unsubscribe) unsubscribe();
  unsubscribe = null;
  for (const watchers of backgroundWatchers.values()) watchers.forEach((watcher) => watcher.dispose());
  backgroundWatchers.clear();
  if (stopModeWatch) stopModeWatch();
  stopModeWatch = null;
  if (visibilityWired && typeof document !== "undefined") {
    document.removeEventListener("visibilitychange", onVisibilityChange);
    visibilityWired = false;
  }
  for (const watcher of entityWatchers.values()) watcher.dispose();
  entityWatchers.clear();
  refreshing.clear();
  warmed.clear();
  activeRows = new Map();
  holdingLock = false;
  if (releaseLock) releaseLock();
  releaseLock = null;
}
