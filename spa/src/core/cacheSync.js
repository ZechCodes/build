// The cache's sync layer: the one reader of the wire.
//
// Every surface paints from the cache and subscribes to it. Nothing above this
// module calls the bridge for a read, and nothing here runs on a timer. Two
// things fill the cache:
//
//   the ordered sync   a bounded pass, in a fixed order, when a session is
//                      greeted, when one reconnects, and when the tab comes
//                      back. Never a full pull: every read past the lists is
//                      either a small whole shape or a cursored delta.
//   the pushes         three subscriptions per device carry bodies, and this
//                      module writes them where the views are already looking.
//
// One tab per browser holds the sync lock and does all of it; every other tab
// reads the same database and hears the writes (core/localCache.js).
//
// Everything here is fire-and-forget against the cache and forgiving of the
// wire: a failed read is a cold record, never an error the user sees.
//
// # The order, and why it is an order
//
// A pass reads the lists first, then the workspace the reader is standing in,
// then the rest in inbox order — so the first thing on screen is filled first
// and the reader's own workspace never waits behind eleven others. Only the
// routed workspace's reads are foreground; everything else rides behind
// whatever a surface is waiting on.

import { App } from "../app.js";
import { contextFor, liveContexts, onDeviceStateChanged } from "./deviceContexts.js";
import { watchChanges } from "./changeEvents.js";
import { cacheableEntityIds, inboxEntries, isFinishedState, routedEntityId } from "./inbox.js";
import { entityIdOf } from "./entityId.js";
import { liveFeedSnapshot, stampProject, stampRow, stampWorkspace, workspaceSummaries } from "./feedMerge.js";
import { mergeActivityDigests } from "./activityDigest.js";
import { THREAD_RECORD_KIND, threadItemKey, windowFromThreadPayload } from "./thread.js";
import {
  cachedAddresses,
  cachedEntityIds,
  cachedSubKeys,
  deleteCached,
  evictEntity,
  readCached,
  readCachedMany,
  writeCached,
} from "./localCache.js";
import { evictWorkspaceData, expireWorkspaceData, isWorkspaceDataKind, withinBytes } from "./cacheLifetime.js";
import { coordinatedRead, requestPriorityFields, rpcReadKey } from "./readRequests.js";
import { pageVisible } from "./visibility.js";

const SYNC_LOCK = "build.cacheSync";

// ─── The thresholds, SPA side (plan README) ──────────────────────────────────

/** Commits read when the cache holds no cursor to read forward from. */
export const LATEST_COMMITS = 20;

/** Conversation items read when the cache holds no sequence to read after. */
export const LATEST_THREAD_ITEMS = 100;

/** Unpushed commits whose patches are kept. */
export const UNPUSHED_COMMITS_MAX = 20;

/** The largest patch worth keeping, asked for on the wire and checked again
 *  here — a bridge that ignored `max_bytes` must not put megabytes under a
 *  record this side says is 256 KB. */
export const COMMIT_PATCH_MAX_BYTES = 262144;

/** How long the background tier holds a flush before sending it. */
export const BACKGROUND_COOLDOWN_MS = 30000;

// ─── What this module holds ──────────────────────────────────────────────────

let holdingLock = false;
let releaseLock = null;
let visibilityWired = false;
let stopDeviceWatch = null;
const syncedSessions = new Map(); // deviceId → the session its last pass ran on
const syncing = new Set(); // deviceIds mid-pass, so triggers never stack
const subscriptions = new Map(); // deviceId → its three watchers

/** A subscription hears rather than polls: there is nothing behind it to run. */
const NOTHING = () => {};

/** What this layer needs of a device's context, captured once per pass. */
const syncContext = (context) =>
  context && {
    deviceId: context.deviceId,
    call: context.rpc,
    requestScope: context.cacheScope,
    active: () => context.active(),
  };

const addressOf = (context, entityId, kind, sub = "") => ({ deviceId: context.deviceId, entityId, kind, sub });

const heldValue = async (context, entityId, kind, sub = "") =>
  (await readCached(addressOf(context, entityId, kind, sub)))?.value;

/** One read, written through by the caller. A failure is a cold record: the
 *  machine is offline, the checkout moved under the read, or this bridge does
 *  not serve the verb. The next trigger or push asks again. */
async function ask(context, method, params, priority) {
  try {
    return await context.call(method, params, requestPriorityFields(priority));
  } catch {
    return null;
  }
}

/** The git scope a feed row's checkout answers under — the same derivation the
 *  branch surface makes, minus the project's own directory: a row that names
 *  neither a run nor a worktree holds no checkout, so it never reaches here. */
function gitScopeOf(row) {
  if (!row || row.kind === "issue") return null;
  if (row.run_id) return { run_id: row.run_id };
  if (row.project_id && row.worktree_id) return { project_id: row.project_id, worktree_id: row.worktree_id };
  return null;
}

/** The scope this row's terminals are listed under. An issue's agent runs in
 *  the project's own checkout, which the project alone names, so this reaches
 *  further than the git scope does. */
function terminalScopeOf(row) {
  if (!row) return null;
  if (row.workspace_id) return { workspace_id: row.workspace_id };
  return gitScopeOf(row) || (row.project_id ? { project_id: row.project_id } : null);
}

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

// ─── The ordered sync ────────────────────────────────────────────────────────

/**
 * One device's whole pass: lists, then every workspace worth reading in the
 * order the reader will want them, then the lifetime rules, then the three
 * subscriptions.
 *
 * Re-entrant by device and no more: a tab that comes back while a pass is
 * running does not start a second one, and another device's pass is another
 * device's business.
 *
 * Answers whether the pass got all the way to the subscriptions — what the
 * caller needs to know to decide whether this session has been read at all.
 */
export async function syncDevice(deviceId) {
  if (!holdingLock || syncing.has(deviceId)) return false;
  syncing.add(deviceId);
  try {
    return await orderedSync(deviceId);
  } finally {
    syncing.delete(deviceId);
  }
}

async function orderedSync(deviceId) {
  const context = await greetedContext(deviceId);
  if (!context) return false;
  const view = await readLists(context);
  if (!view || !context.active()) return false;
  const pass = await workspacesToRead(context, view);
  await readWorkspaces(context, pass);
  await evictRowsThatAreOver(context, view);
  await dropWhatTheBoardStoppedNaming(context, view);
  if (!context.active()) return false;
  await subscribeDevice(context);
  return true;
}

/** This device, once its bridge has said what it speaks. A bridge says which
 *  API major it answers in in its greeting, and until that has settled,
 *  asking it anything is asking for an answer in a shape this tab may not be
 *  able to read. Null where a reconnect landed under the wait. */
async function greetedContext(deviceId) {
  const greeting = contextFor(deviceId)?.greeted;
  await greeting;
  if (greeting !== contextFor(deviceId)?.greeted) return null;
  const context = syncContext(contextFor(deviceId));
  return context?.active() ? context : null;
}

async function readWorkspaces(context, pass) {
  for (const entityId of pass.order) {
    if (!context.active()) return;
    await syncWorkspace(context, entityId, pass.rows.get(entityId), entityId === pass.routed);
  }
}

/** Step 1: the three lists, written as they land so the inbox paints before
 *  any workspace has been read. A device whose bridge does not serve
 *  workspaces still has a board. */
async function readLists(context) {
  const [board, projects, workspaces] = await Promise.all([
    ask(context, "board.list", {}, "background"),
    ask(context, "project.list", {}, "background"),
    ask(context, "workspace.list", {}, "background"),
  ]);
  if (!board || !projects || !context.active()) return null;
  const view = liveFeedSnapshot(board, projects, workspaces || { workspaces: [] }, context.deviceId);
  await writeLists(context, view);
  return view;
}

async function writeLists(context, view) {
  await writeCached(addressOf(context, "", "feed"), view);
  await writeCached(addressOf(context, "", "projects"), view.projects);
  await writeCached(addressOf(context, "", "workspaces"), view.workspaces);
  for (const row of view.items || []) {
    const entityId = entityIdOf(row);
    if (entityId) await writeCached(addressOf(context, entityId, "row"), row);
  }
}

/**
 * Step 2: which workspaces this pass reads, and in what order.
 *
 * The active set is the inbox's own partition, plus the Recent rows that still
 * hold data — and a Recent row is aged out before it is asked that question, so
 * "still holds data" means "was written to inside the TTL", which is the rule.
 * The routed workspace leads, whatever the inbox order says.
 */
async function workspacesToRead(context, view) {
  const items = view.items || [];
  const rows = new Map();
  for (const row of items) {
    const entityId = entityIdOf(row);
    if (entityId) rows.set(entityId, row);
  }
  const active = cacheableEntityIds({ items });
  const recent = await recentStillHoldingData(context, items, new Set(active));
  const routed = routedEntityId(App.route, view);
  const order = [...new Set([...(routed ? [routed] : []), ...active, ...recent])].filter((id) => rows.has(id));
  return { order, rows, routed };
}

/** The Recent rows worth reading: aged out first, then asked what survived.
 *  A row whose data all expired is left cold — the reader has not been near it
 *  for three days, and a push will fill it if they go back. */
async function recentStillHoldingData(context, items, seen) {
  const kept = [];
  for (const entry of inboxEntries({ items }).recent) {
    const entityId = entry.entityId;
    if (!entityId || seen.has(entityId) || !context.active()) continue;
    seen.add(entityId);
    await expireWorkspaceData(context.deviceId, entityId);
    const left = await cachedAddresses({ deviceId: context.deviceId, entityId });
    if (left.some((address) => isWorkspaceDataKind(address.kind))) kept.push(entityId);
  }
  return kept;
}

/** Step 4: a row the board still lists but whose work is over keeps nothing.
 *  The push that said so evicts it as it lands — but a tab that was not open
 *  to hear it boots to a board that already says the row is over, and the rule
 *  has to hold there too. */
async function evictRowsThatAreOver(context, view) {
  for (const row of view.items || []) {
    if (!context.active()) return;
    const entityId = entityIdOf(row);
    if (entityId && isFinishedState(row.state)) await evictWorkspaceData(context.deviceId, entityId);
  }
}

/** Step 4, the other half: an entity the board has stopped naming altogether
 *  is gone — the row is the board's to list and the board's to remove, so the
 *  row goes with the data. Another device's records are another device's
 *  business. */
async function dropWhatTheBoardStoppedNaming(context, view) {
  const named = new Set();
  for (const row of view.items || []) {
    const entityId = entityIdOf(row);
    if (entityId) named.add(entityId);
  }
  for (const cachedId of await cachedEntityIds(context.deviceId)) {
    if (!context.active()) return;
    if (!named.has(cachedId)) await evictEntity(context.deviceId, cachedId);
  }
}

// ─── Step 3: one workspace ───────────────────────────────────────────────────

/** Everything one workspace holds, in the order a reader opening it wants it:
 *  what changed, where the files are, the shells, the commit lists, the
 *  conversations, and last the big bodies — the patch behind each unpushed
 *  commit and the working tree's diff. Those last are a quarter of a megabyte
 *  each and up to twenty of them; nothing is looking at them until a reviewer
 *  opens the changes, and the thread is on screen the moment the workspace
 *  is. */
async function syncWorkspace(context, entityId, row, routed) {
  const priority = routed ? "foreground" : "background";
  const scope = gitScopeOf(row);
  if (scope) {
    await syncStatus(context, entityId, scope, priority);
    await syncTrees(context, entityId, scope, priority);
  }
  await syncTerminals(context, entityId, row, priority);
  const unpushed = scope ? await syncCommits(context, entityId, scope, priority) : null;
  await syncThreads(context, entityId, row, priority);
  if (!scope) return;
  await syncPatches(context, entityId, scope, unpushed, priority);
  await syncWorkingDiff(context, entityId, row, priority);
}

/** The status, asked conditionally: the `status_key` the cache holds is the
 *  same key `git.status` answers with, so holding it is proof there is nothing
 *  to fetch and the answer says only that. */
async function syncStatus(context, entityId, scope, priority) {
  const held = await heldValue(context, entityId, "status");
  const params = held?.status_key ? { ...scope, if_status_key: held.status_key } : scope;
  const answer = await ask(context, "git.status", params, priority);
  if (!answer || answer.unchanged || !context.active()) return;
  await writeCached(addressOf(context, entityId, "status"), answer);
}

/** The top-level listing always — the Files tab's first paint — plus whichever
 *  directories the reader has walked into, which are the tree records the cache
 *  already holds. */
async function syncTrees(context, entityId, scope, priority) {
  const walked = await cachedSubKeys(context.deviceId, entityId, "tree");
  await listTrees(context, entityId, scope, new Set(["", ...walked]), priority);
}

async function listTrees(context, entityId, scope, paths, priority) {
  for (const path of paths) {
    if (!context.active()) return;
    const listing = await ask(context, "fs.tree", { ...scope, path }, priority);
    if (!listing || !context.active()) continue;
    await writeCached(
      addressOf(context, entityId, "tree", path),
      { path: listing.path || "", entries: listing.entries || [] },
    );
  }
}

async function syncTerminals(context, entityId, row, priority) {
  const scope = terminalScopeOf(row);
  if (!scope) return;
  const answer = await ask(context, "term.list", scope, priority);
  if (!answer || !context.active()) return;
  await writeCached(addressOf(context, entityId, "terminals"), { tabs: answer.terminals || [] });
}

/** The two commit lists, answered whole. What the unpushed one said is handed
 *  back: the patches behind those commits are read later in the pass, once
 *  the conversations are in. */
async function syncCommits(context, entityId, scope, priority) {
  await syncLog(context, entityId, scope, priority);
  return syncUnpushed(context, entityId, scope, priority);
}

/** The commit list, read forward from the newest hash the cache holds — or the
 *  latest 20 when it holds none. */
async function syncLog(context, entityId, scope, priority) {
  const held = await heldValue(context, entityId, "log");
  const params = held?.newest ? { ...scope, since: held.newest } : { ...scope, limit: LATEST_COMMITS };
  const answer = await ask(context, "git.log", params, priority);
  if (!answer || !context.active()) return;
  await writeCached(addressOf(context, entityId, "log"), mergedLog(held, answer));
}

/**
 * The commit record after an answer.
 *
 * A `reset` means the history under the cursor moved — a rebase, a reset, a
 * hash this checkout has never heard of — so the answer replaces what was
 * held rather than being prepended to it. Anything else is the commits since
 * the cursor, in front of the ones already there, deduplicated by hash.
 *
 * The flag itself is what one answer said and does not go into the record: a
 * reader finding it there later would read it as news about the record.
 */
export function mergedLog(held, answer) {
  const { reset, ...rest } = answer;
  const arriving = answer.commits || [];
  const previous = reset ? [] : (held?.commits || []);
  const arrived = new Set(arriving.map((commit) => commit.hash));
  const commits = [...arriving, ...previous.filter((commit) => !arrived.has(commit.hash))];
  return { ...rest, commits, newest: newestAfter(held, answer, commits) };
}

/**
 * The commit record after a pushed history.
 *
 * A push has no cursor to answer from: a `git` item carries the head of the
 * history as it stands, never the commits since anything. So the record's own
 * commits survive only where that window still reaches the hash the record was
 * reading forward from. Where it does not — a rebase, a reset, or more commits
 * than a window holds landing at once — the held commits cannot be shown to be
 * behind the window's, and a whole recent history is worth more than an older
 * one with a hole in it that nothing will ever come back for.
 */
export function windowedLog(held, window) {
  const reaches = (window.commits || []).some((commit) => commit.hash === held?.newest);
  return mergedLog(reaches ? held : null, window);
}

/** The cursor the record reads forward from next: what the answer named, else
 *  the newest commit the record is left holding, else the cursor it already
 *  had.
 *
 *  A reset takes that last one with it. The hash this cache was reading from
 *  is one the checkout no longer has, so keeping it would have the next read
 *  ask after it again and be answered `reset` again, for ever. With no cursor
 *  the next read asks for the latest commits, which is what a cache that
 *  knows nothing of a history asks for. */
const newestAfter = (held, answer, commits) =>
  answer.newest || commits[0]?.hash || (answer.reset ? null : held?.newest || null);

/** The unpushed commits, without the patch that rides with them: the Records
 *  table holds the base, the commit list and the diff key, and the patch
 *  behind each commit is its own record under its own cap. */
const unpushedRecord = (answer) => {
  const record = { ...answer };
  delete record.patch;
  return record;
};

async function syncUnpushed(context, entityId, scope, priority) {
  const answer = await ask(context, "git.unpushed", scope, priority);
  if (!answer || !context.active()) return null;
  await writeCached(addressOf(context, entityId, "unpushed"), unpushedRecord(answer));
  return answer;
}

/** The patches behind the unpushed commits: the first twenty, and only the
 *  ones not already held. A patch too big for the record is not stored —
 *  the reader opens it and gets it off the wire, which is what the truncated
 *  answer says on screen anyway. */
async function syncPatches(context, entityId, scope, unpushed, priority) {
  const hashes = (unpushed?.commits || [])
    .map((commit) => commit.hash)
    .filter(Boolean)
    .slice(0, UNPUSHED_COMMITS_MAX);
  const held = await cachedSubKeys(context.deviceId, entityId, "patch");
  await dropStalePatches(context, entityId, held, new Set(hashes));
  for (const hash of hashes) {
    if (held.includes(hash) || !context.active()) continue;
    const answer = await ask(context, "git.show", { ...scope, hash, max_bytes: COMMIT_PATCH_MAX_BYTES }, priority);
    if (!answer || !context.active() || !withinBytes(answer.patch, COMMIT_PATCH_MAX_BYTES)) continue;
    await writeCached(addressOf(context, entityId, "patch", hash), answer);
  }
}

/** A patch for a commit that is no longer unpushed has been published: it is
 *  in the log like every other commit, and nobody is reviewing it here. */
async function dropStalePatches(context, entityId, held, wanted) {
  const stale = held.filter((hash) => !wanted.has(hash));
  if (stale.length) await deleteCached(stale.map((hash) => addressOf(context, entityId, "patch", hash)));
}

/** Which verb answers this row's working-tree diff, and what to name the read
 *  so a view asking for the same body shares it. */
function diffRead(row, held) {
  const runId = row.run_id;
  const method = runId ? "run.diff" : "worktree.diff";
  const repository = runId ? `run:${runId}` : `worktree:${row.project_id}:${row.worktree_id}`;
  const base = runId ? { run_id: runId } : { project_id: row.project_id, worktree_id: row.worktree_id };
  return { method, repository, params: held?.diff_key ? { ...base, if_diff_key: held.diff_key } : base };
}

/** The working-tree diff is the largest thing a workspace holds, and a push
 *  carries it whenever it fits — so a pass asks for it only where the cache
 *  has none at all. */
async function syncWorkingDiff(context, entityId, row, priority) {
  if (await readCached(addressOf(context, entityId, "diff"))) return;
  await pullWorkingDiff(context, entityId, row, priority);
}

async function pullWorkingDiff(context, entityId, row, priority) {
  const address = addressOf(context, entityId, "diff");
  const held = (await readCached(address))?.value;
  const { method, repository, params } = diffRead(row, held);
  const key = rpcReadKey({
    deviceId: context.deviceId,
    requestScope: context.requestScope,
    repository,
    call: context.call,
    method,
    params,
  });
  const diff = await coordinatedRead({
    key,
    priority,
    load: (envelope) => context.call(method, params, envelope),
  }).catch(() => null);
  if (!diff || diff.unchanged || !context.active()) return;
  await writeCached(address, diffRecord(held, diff, row));
}

/** The diff record after a new body, wherever the body came from. The body
 *  replaces what was held; the two fields the review surface keeps beside it —
 *  which project the diff belongs to, and what the reviewer has triaged in it —
 *  are nothing the wire knows about and stay where they were put. */
const diffRecord = (held, diff, row) => ({
  ...held,
  ...diff,
  triage: held?.triage || null,
  projectId: row?.project_id || held?.projectId || null,
});

/** Every conversation on this workspace. The feed row's `agents[]` IS the
 *  conversation list, so a conversation nobody has opened is still read once
 *  and paints instantly the first time it is. */
async function syncThreads(context, entityId, row, priority) {
  for (const agent of row?.agents || []) {
    if (!context.active()) return;
    await syncThread(context, entityId, agent, priority);
  }
}

/** One conversation, read forward from the sequence the cache holds — or the
 *  latest hundred where it holds none. */
async function syncThread(context, entityId, agent, priority) {
  const sub = agent.conversation_id || agent.id || "";
  if (!sub) return;
  const address = addressOf(context, entityId, THREAD_RECORD_KIND, sub);
  const held = (await readCached(address))?.value;
  const after = Number(held?.deliveredSequence || 0);
  const page = await ask(context, "thread.page", threadPageParams(entityId, agent, after), priority);
  if (!page || !context.active()) return;
  const window = threadWindow(held, page);
  if (window) await writeCached(address, window);
}

const threadPageParams = (entityId, agent, after) => ({
  entity_id: entityId,
  ...(agent.id ? { agent_id: agent.id } : {}),
  ...(after ? { after_sequence: after } : {}),
  limit: LATEST_THREAD_ITEMS,
});

/**
 * The saved window after a page.
 *
 * With nothing held, the page IS the window: it is the latest hundred items,
 * and `has_more` on it means the conversation reaches back further than the
 * window does. With a window held, the page is a forward delta — items after
 * the cursor — so it is appended, the cursor moves to the newest sequence, and
 * how far back the window reaches is what it always was.
 */
export function threadWindow(held, page) {
  const arrived = windowFromThreadPayload(page);
  if (!held) return arrived;
  if (!arrived) return null; // nothing new: the record stands
  const seen = new Set(held.items.map(threadItemKey));
  return {
    ...held,
    items: [...held.items, ...arrived.items.filter((item) => !seen.has(threadItemKey(item)))],
    deliveredSequence: Math.max(Number(held.deliveredSequence || 0), arrived.deliveredSequence),
    knownTotalItems: arrived.knownTotalItems ?? held.knownTotalItems ?? null,
    activityDigests: mergeActivityDigests(held.activityDigests || [], page),
  };
}

// ─── Step 5: the three subscriptions ─────────────────────────────────────────
//
// `s-inbox` carries every workspace's state and conversation in realtime —
// that is the inbox, and it is what the reader is looking at whatever page
// they are on. `s-background` carries every workspace's git, files and shells
// on a 30 s cooldown. `s-active` carries the same kinds for the one workspace
// the reader is standing in, in realtime, and is re-issued when they move.

const subscriptionShape = (deviceId) => ({
  deviceId,
  refresh: NOTHING,
  catchUpOnVisible: false,
  // A push carries bodies, so a hidden tab writing them is a hidden tab that
  // paints instantly when it comes back. Nothing here is a read to defer.
  pausesWhileHidden: false,
  onChanges: (items) => void applyChanges(items, deviceId),
});

/** The three watchers, and the one that follows the reader.
 *
 *  The routed workspace is resolved here rather than carried from the top of
 *  the pass: a pass is six or eight reads per workspace long and the reader
 *  walks off mid-way through it. Standing the active subscription up on where
 *  they were when it started would leave the workspace on screen with no
 *  realtime watcher at all. */
async function subscribeDevice(context) {
  const deviceId = context.deviceId;
  if (!subscriptions.has(deviceId)) {
    const shape = subscriptionShape(deviceId);
    subscriptions.set(deviceId, {
      inbox: watchChanges({
        ...shape,
        id: "s-inbox",
        scope: "all",
        kinds: ["state", "thread"],
        mode: "realtime",
        priority: "foreground",
      }),
      background: watchChanges({
        ...shape,
        id: "s-background",
        scope: "all",
        kinds: ["git", "files", "terminals"],
        mode: { batch_ms: BACKGROUND_COOLDOWN_MS },
        priority: "background",
      }),
      active: null,
      activeId: null,
    });
  }
  await refollowRoute(deviceId);
}

/** The active subscription follows the reader: the workspace they are standing
 *  in is watched in realtime, and the one they left falls back to the
 *  background tier's cooldown. A route that names no workspace on this device
 *  leaves it with none. */
function followRoutedEntity(deviceId, entityId) {
  const held = subscriptions.get(deviceId);
  const wanted = entityId || null;
  if (!held || held.activeId === wanted) return;
  held.active?.dispose();
  held.active = null;
  held.activeId = wanted;
  if (!wanted) return;
  held.active = watchChanges({
    ...subscriptionShape(deviceId),
    id: "s-active",
    entity: wanted,
    kinds: ["git", "files", "terminals"],
    mode: "realtime",
    priority: "foreground",
  });
}

/** The reader moved. Called from the one place every route is taken up
 *  (app.js `standOn`), so a move within a surface counts the same as a
 *  navigation: both can change which workspace is on screen.
 *
 *  A device whose pass has not reached its subscriptions yet is not here to be
 *  told, and does not need to be: that pass resolves the route for itself when
 *  it gets there. */
export function routeChanged() {
  if (!holdingLock) return;
  for (const deviceId of [...subscriptions.keys()]) void refollowRoute(deviceId);
}

async function refollowRoute(deviceId) {
  if (!contextFor(deviceId) || !subscriptions.has(deviceId)) return;
  const view = await routeView(deviceId);
  if (!subscriptions.has(deviceId)) return;
  followRoutedEntity(deviceId, routedEntityId(App.route, view));
}

/** The snapshot a route is resolved against: every row this device holds right
 *  now, and the two lists a workspace route is named by.
 *
 *  Off the records rather than off the `feed` the last pass wrote, because a
 *  workspace created since that pass rode in on its own `state` item — the
 *  board pushes deltas, not the whole board — and lives in the cache as a row
 *  and nowhere else. That workspace is the likeliest of all to be the one
 *  being stood on: the reader just made it and walked in. */
async function routeView(deviceId) {
  const rowAddresses = (await cachedAddresses({ deviceId })).filter((address) => address.kind === "row");
  const [rows, projects, workspaces] = await Promise.all([
    readCachedMany(rowAddresses),
    readCached({ deviceId, entityId: "", kind: "projects" }),
    readCached({ deviceId, entityId: "", kind: "workspaces" }),
  ]);
  return {
    items: rows.map((record) => record?.value).filter(Boolean),
    projects: projects?.value || [],
    workspaces: workspaces?.value || [],
  };
}

// ─── Applying a push ─────────────────────────────────────────────────────────

const BOARD_ENTITY = "board";

async function applyChanges(items, deviceId) {
  const context = syncContext(contextFor(deviceId));
  if (!context || !holdingLock) return;
  for (const item of items) {
    if (!context.active()) return;
    await applyItem(context, item);
  }
}

async function applyItem(context, item) {
  const entityId = String(item.entity_id || "");
  if (!entityId) return;
  if (entityId === BOARD_ENTITY) {
    await applyBoard(context, item.state || {});
    return;
  }
  for (const [field, apply] of APPLIERS) {
    if (!context.active()) return;
    if (item[field]) await apply(context, entityId, item[field]);
  }
}

/** The board item: which entities left the board, and the two lists when they
 *  moved. An entity that finished, was deleted, or was cleared away appears in
 *  `removed`, and its data goes at once — the row stays until the next pass
 *  finds the board no longer naming it. */
async function applyBoard(context, state) {
  for (const entityId of state.removed || []) {
    if (!context.active()) return;
    await evictWorkspaceData(context.deviceId, String(entityId));
  }
  if (state.projects) {
    await writeCached(
      addressOf(context, "", "projects"),
      state.projects.map((project) => stampProject(project, context.deviceId)),
    );
  }
  if (state.workspaces) {
    // The item's list carries no summaries — `board.list` is the only read
    // that answers what a workspace has to show and whether it can be
    // finished — so the verdicts the cache holds are stamped back on.
    const summaries = workspaceSummaries(await heldValue(context, "", "workspaces"));
    await writeCached(
      addressOf(context, "", "workspaces"),
      state.workspaces.map((workspace) => stampWorkspace(workspace, context.deviceId, summaries)),
    );
  }
}

/** `state`: the feed row exactly as `board.list` carries it. A row whose work
 *  is over takes the workspace's data with it — nobody is coming back to it. */
async function applyState(context, entityId, state) {
  await writeCached(addressOf(context, entityId, "row"), stampRow(state, context.deviceId));
  if (isFinishedState(state.state)) await evictWorkspaceData(context.deviceId, entityId);
}

/** `thread`: one tip per conversation, carrying the items since this
 *  subscription's last flush. */
async function applyThreadItem(context, entityId, tips) {
  for (const tip of tips || []) {
    if (!context.active()) return;
    await applyThreadTip(context, entityId, tip);
  }
}

/** Which conversation a tip is about, addressed the way the record is. */
const tipKey = (tip) => tip.conversation_id || tip.agent_id || "";

/** Whether a tip says anything the record does not already hold. A
 *  conversation this cache has never held is always news. */
const tipIsNews = (tip, held) => !held || Number(tip.last_sequence || 0) > Number(held.deliveredSequence || 0);

/** Whether a tip's items carry on from where the record stands. `since_sequence`
 *  is the sequence they run from, exclusive — the cursor the subscription had
 *  when it sent them. Past this record's own cursor, the items in between went
 *  to a flush this cache did not write (the page that answered it failed), and
 *  appending would move the cursor over the hole and leave it there for good:
 *  every later read asks after the newest sequence. */
const tipRunsOnFromRecord = (tip, held) =>
  Number(tip.since_sequence || 0) <= Number(held.deliveredSequence || 0);

async function applyThreadTip(context, entityId, tip) {
  const sub = tipKey(tip);
  if (!sub) return;
  const address = addressOf(context, entityId, THREAD_RECORD_KIND, sub);
  const held = (await readCached(address))?.value;
  if (!tipIsNews(tip, held)) return;
  // A burst past the push cap arrives as a tip with no items, a conversation
  // nothing is held for has nothing to append to, and a tip that starts past
  // the cursor would append over a gap. All three are one cursored page, which
  // is the read this layer would have made anyway.
  const items = held && tipRunsOnFromRecord(tip, held) ? (tip.items || []) : [];
  if (!items.length) {
    await syncThread(context, entityId, { id: tip.agent_id, conversation_id: tip.conversation_id }, "background");
    return;
  }
  const window = threadWindow(held, { items, thread_total: tip.thread_total });
  if (window) await writeCached(address, window);
}

/** `git`: the shapes ride the push, so nothing is asked for them. Two things
 *  never ride one — the patch behind a commit, and a working-tree diff too big
 *  to send — and those are pulled here. */
async function applyGit(context, entityId, git) {
  const row = await heldValue(context, entityId, "row");
  if (git.status) await writeCached(addressOf(context, entityId, "status"), git.status);
  if (git.log) {
    await writeCached(addressOf(context, entityId, "log"), windowedLog(await heldValue(context, entityId, "log"), git.log));
  }
  if (git.unpushed) await writeCached(addressOf(context, entityId, "unpushed"), unpushedRecord(git.unpushed));
  if (git.diff) await writePushedDiff(context, entityId, git.diff, row);
  await pullWhatTheGitItemCouldNotCarry(context, entityId, git, row);
}

async function writePushedDiff(context, entityId, diff, row) {
  const address = addressOf(context, entityId, "diff");
  await writeCached(address, diffRecord((await readCached(address))?.value, diff, row));
}

async function pullWhatTheGitItemCouldNotCarry(context, entityId, git, row) {
  const scope = gitScopeOf(row);
  if (!scope) return;
  if (git.unpushed) await syncPatches(context, entityId, scope, git.unpushed, "background");
  // A null diff is one the bridge had and could not send. It is only worth a
  // round trip for the workspace on screen; the rest read it when opened.
  if (git.diff === null && subscriptions.get(context.deviceId)?.activeId === entityId) {
    await pullWorkingDiff(context, entityId, row, "foreground");
  }
}

/** `files`: the root listing rides the push. The deeper ones the reader walked
 *  into are re-listed where a changed path sits in them — and all of them when
 *  the list was truncated, because a truncated list means "refetch the tree",
 *  not "these paths". */
async function applyFiles(context, entityId, files) {
  if (files.root) {
    await writeCached(
      addressOf(context, entityId, "tree", files.root.path || ""),
      { path: files.root.path || "", entries: files.root.entries || [] },
    );
  }
  const scope = gitScopeOf(await heldValue(context, entityId, "row"));
  if (!scope) return;
  const walked = await cachedSubKeys(context.deviceId, entityId, "tree");
  const stale = files.truncated ? walked : dirsOf(files.paths).filter((dir) => walked.includes(dir));
  await listTrees(context, entityId, scope, new Set(files.root ? stale : ["", ...stale]), "background");
}

const applyTerminals = (context, entityId, terminals) =>
  writeCached(addressOf(context, entityId, "terminals"), { tabs: terminals.tabs || [] });

/** One writer per kind, in the order a reader would want them applied: what
 *  the row says, what was said in it, then the surfaces under it. */
const APPLIERS = [
  ["state", applyState],
  ["thread", applyThreadItem],
  ["git", applyGit],
  ["files", applyFiles],
  ["terminals", applyTerminals],
];

// ─── Standing up and standing down ───────────────────────────────────────────

/** Take the browser-wide sync lock, or queue for it. The holder does the whole
 *  job; every other tab reads the same database and hears the writes. Without a
 *  Locks API (jsdom, old browsers) this tab just syncs. */
function acquireLock() {
  const locks = typeof navigator !== "undefined" ? navigator.locks : undefined;
  if (!locks) {
    takeLock();
    return;
  }
  locks
    .request(SYNC_LOCK, () => {
      takeLock();
      return new Promise((resolve) => {
        releaseLock = resolve;
      });
    })
    .catch(() => {
      /* the lock died with the tab that held it; stopCacheSync resolves ours */
    });
}

/** The lock is ours: whatever is already open gets its pass now. The grant can
 *  arrive long after `startCacheSync` — another tab held it — so the devices
 *  are considered here rather than at start. */
function takeLock() {
  holdingLock = true;
  considerDevices();
}

/** Every device that can answer, synced once per session it is on: its first
 *  greeting, and every reconnect. A new session has a gap behind it that
 *  announced nothing, so it is read whatever the last one said. */
function considerDevices() {
  if (!holdingLock) return;
  for (const context of liveContexts()) {
    if (syncedSessions.get(context.deviceId) === context.session) continue;
    syncedSessions.set(context.deviceId, context.session);
    void syncSessionOnce(context.deviceId, context.session);
  }
}

/** A session is marked read before its pass runs, so two announcements in a
 *  row are one pass. A pass that did not finish takes the mark off again: the
 *  lists it stopped at are the step the subscriptions sit behind, so a session
 *  left marked on a stalled `board.list` would spend its whole life hearing
 *  nothing and reading nothing. Unmarked, the next thing the device announces
 *  asks again. */
async function syncSessionOnce(deviceId, session) {
  if (await syncDevice(deviceId)) return;
  if (syncedSessions.get(deviceId) === session) syncedSessions.delete(deviceId);
}

/** Coming back to the tab: everything the subscriptions could not say while it
 *  was away is whatever a pass reads, so every open device gets one. */
function onVisibilityChange() {
  if (!pageVisible() || !holdingLock) return;
  for (const context of liveContexts()) void syncDevice(context.deviceId);
}

export function startCacheSync() {
  stopCacheSync();
  stopDeviceWatch = onDeviceStateChanged(considerDevices);
  if (typeof document !== "undefined" && !visibilityWired) {
    document.addEventListener("visibilitychange", onVisibilityChange);
    visibilityWired = true;
  }
  acquireLock();
}

export function stopCacheSync() {
  if (stopDeviceWatch) stopDeviceWatch();
  stopDeviceWatch = null;
  for (const held of subscriptions.values()) {
    held.inbox.dispose();
    held.background.dispose();
    held.active?.dispose();
  }
  subscriptions.clear();
  syncedSessions.clear();
  syncing.clear();
  if (visibilityWired && typeof document !== "undefined") {
    document.removeEventListener("visibilitychange", onVisibilityChange);
    visibilityWired = false;
  }
  holdingLock = false;
  if (releaseLock) releaseLock();
  releaseLock = null;
}
