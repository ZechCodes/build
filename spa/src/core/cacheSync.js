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
//
// # Subscribed before it reads
//
// The bridge starts a subscription empty and records a change only for the
// subscriptions it holds when the change happens. So a pass takes its
// subscriptions out before it asks for anything, and whatever changes after
// the bridge holds them is pushed (#142). A subscription that lands after the
// pass's reads were asked — a device with nothing cached reads at once, and a
// subscribe can outlast the wait — leaves a gap behind it, so its landing is
// followed by another pass (`subscriptionLanded`). A push can overtake the
// read it followed, so a read never writes over a record a push wrote after it
// was asked, or under an entity a push took away since (core/pushFence.js).

import { trailingRead } from "./trailingRead.js";
import { App } from "../app.js";
import { contextFor, liveContexts, onDeviceStateChanged } from "./deviceContexts.js";
import { onSubscriptionHeld, subscriptionsSettledFor, watchChanges } from "./changeEvents.js";
import { forgetPushes, notePush, pushFence, pushedSince, removedSince } from "./pushFence.js";
import { cacheableEntityIds, inboxEntries, isFinishedState, routedEntityId } from "./inbox.js";
import { cachedRouteEntityId } from "./cachedRows.js";
import { entityIdOf } from "./entityId.js";
import { FEED_COLLECTIONS, liveFeedSnapshot, stampProject, stampRow, stampWorkspace, workspaceSummaries } from "./feedMerge.js";
import { THREAD_RECORD_KIND } from "./thread.js";
import { syncThreadWindow, threadWindow } from "./threadSync.js";
import { replaceSessionList, updateSessionSummary } from "./sessionListCache.js";
import {
  cachedAddresses,
  cachedEntityIds,
  cachedSubKeys,
  deleteCached,
  evictEntity,
  mergeCached,
  readCached,
  updateCachedFeed,
  writeCached,
} from "./localCache.js";
import { ISSUE_RECORD_KIND } from "./issueCache.js";
import { issuesRecord, readIssuesRecord, writeIssuesRecord } from "./trackerCache.js";
import { writeUserSession } from "./userSessionCache.js";
import { inboxPushKinds } from "./trackerPush.js";
import {
  FILE_RECORD_KIND,
  cacheFileBody,
  evictWorkspaceData,
  expireWorkspaceData,
  isWorkspaceDataKind,
  withinBytes,
} from "./cacheLifetime.js";
import {
  surfaceSessionGeneration,
  surfacesCacheAddress,
  surfacesFingerprint,
  surfacesRecord,
} from "./surfacesCache.js";
import { coordinatedRead, requestPriorityFields, rpcReadKey } from "./readRequests.js";
import { pageVisible } from "./visibility.js";
import { writeUsageLimits } from "./usageLimits.js";
import {
  BACKGROUND_COOLDOWN_MS,
  COMMIT_PATCH_MAX_BYTES,
  LATEST_COMMITS,
  LATEST_THREAD_ITEMS,
  PATCH_RECORD_KIND,
  UNPUSHED_COMMITS_MAX,
  WORKING_DIFF_MAX_BYTES,
} from "./cacheThresholds.js";

export { threadWindow };

const SYNC_LOCK = "build.cacheSync";

/** How long this tab queues for the sync lock before doing the job anyway.
 *
 *  A phone freezes a backgrounded tab where it stands, and a frozen tab holding
 *  this lock hands it back to nobody: waiting on it for ever is a tab that
 *  never reads a thing. So the wait is bounded, and past it this tab syncs too.
 *  Two tabs syncing costs a duplicate read of each shape — what two open tabs
 *  always cost — and a tab that never syncs costs the reader the whole app. */
export const LOCK_WAIT_MS = 4000;

/** How long a pass waits for the bridge to say which API major it speaks.
 *
 *  A greeting settles when the bridge answers, when the session dies, or when a
 *  newer session arms its own — but a session whose transport is up and whose
 *  bridge never answers settles none of those, and a pass awaiting it would
 *  hold this device's turn for the life of the tab. Past the wait the pass
 *  stands down; the greeting landing announces the device, which asks again. */
export const GREETING_WAIT_MS = 15000;
const RESTORE_DRAIN_MS = 1000;

// The thresholds this layer is written against live in core/cacheThresholds.js
// — a module that imports nothing, so the surfaces can read the same bounds
// without importing this one. Re-exported here because this is where a reader
// looking for them expects them.
export {
  BACKGROUND_COOLDOWN_MS,
  COMMIT_PATCH_MAX_BYTES,
  LATEST_COMMITS,
  LATEST_THREAD_ITEMS,
  PATCH_RECORD_KIND,
  UNPUSHED_COMMITS_MAX,
} from "./cacheThresholds.js";

// ─── What this module holds ──────────────────────────────────────────────────

let holdingLock = false;
let releaseLock = null;
let lockWait = null; // the bounded queue for the lock, while it is running
let visibilityWired = false;
let stopDeviceWatch = null;
let stopLandingWatch = null;
const syncedSessions = new Map(); // deviceId → the session its last pass ran on
const passes = new Map(); // deviceId → the pass running on it, so triggers never stack
const subscriptions = new Map(); // deviceId → its three watchers
const restoredScopes = new Map(); // deviceId → fresh read identity for an ICE-restored session

/** A subscription hears rather than polls: there is nothing behind it to run. */
const NOTHING = () => {};

/** What this layer needs of a device's context, captured once per pass.
 *
 *  A pass is over the moment its own session is not the device's any more:
 *  `superseded` is how a newer session's pass tells this one to stop, and it
 *  rides `active()` so every step that already asks "is this still worth
 *  doing" asks this too. */
const requestScopeOf = (context) => {
  const restored = restoredScopes.get(context.deviceId);
  if (restored?.session === context.session) return restored.scope;
  if (restored) restoredScopes.delete(context.deviceId);
  return context.cacheScope;
};

const syncContext = (context, turn = null) => {
  if (!context) return null;
  const session = context.session;
  const requestScope = requestScopeOf(context);
  return {
    deviceId: context.deviceId,
    call: context.rpc,
    requestScope,
    active: () => context.active() && context.session === session
      && requestScopeOf(context) === requestScope && !turn?.superseded,
  };
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

/** A whole shape read off the wire, written where it belongs unless a push
 *  wrote that record after the read was asked: the push is the newer word
 *  (core/pushFence.js). Answers what was read, written or not. */
async function readWhole(context, address, request, shape = (answer) => answer) {
  const fence = pushFence();
  const answer = await ask(context, request.method, request.params, request.priority);
  if (!answer || answer.unchanged || !context.active()) return answer;
  if (!pushedSince(address, fence)) await writeCached(address, shape(answer));
  return answer;
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
 * One device's whole pass: the three subscriptions, then the lists, then every
 * workspace worth reading in the order the reader will want them, then the
 * lifetime rules.
 *
 * Re-entrant by device and no more: a tab that comes back while a pass is
 * running does not start a second one, and another device's pass is another
 * device's business.
 *
 * Answers whether the pass got all the way through — what the caller needs to
 * know to decide whether this session has been read at all.
 */
export async function syncDevice(deviceId) {
  if (!holdingLock) return false;
  const running = passes.get(deviceId);
  if (running) {
    // The same session asking twice is one pass: a tab coming back while its
    // own pass is out has nothing to add.
    if (running.session === sessionOf(deviceId)) return false;
    // A newer session, though, is a different machine's answer — possibly a
    // different bridge — and the pass out is reading a session that has gone.
    // It is stood down and waited out rather than this ask being dropped: a
    // dropped ask is a device that is never read again, because nothing but
    // another announcement would ever ask, and the announcement that would
    // have is the one being dropped.
    running.superseded = true;
    await running.done;
    if (passes.has(deviceId)) return false; // another ask got in first; it owns this turn
  }
  return startPass(deviceId);
}

/** A recovered path can re-greet the bridge without replacing its session.
 * Its earlier pass may have read the old process, so stand it down and give
 * its writes a bounded chance to drain before reading the recovered bridge.
 * Ordinary device-state changes are covered by the per-session pass. */
export async function syncRestoredDevice(deviceId, session) {
  if (!holdingLock || sessionOf(deviceId) !== session) return false;
  // A carried session has the same caller and cache scope as before its ICE
  // restart. The bridge behind it may be new, so reads from the old path may
  // neither answer nor be shared with this path's baseline.
  restoredScopes.set(deviceId, { session, scope: {} });
  const previous = passes.get(deviceId);
  if (previous) {
    previous.superseded = true;
    let timeout;
    try {
      await Promise.race([
        previous.done,
        new Promise((resolve) => { timeout = setTimeout(resolve, RESTORE_DRAIN_MS); }),
      ]);
    } finally {
      clearTimeout(timeout);
    }
    // A read sent before the restart may never answer. Its captured turn is
    // already fenced off, so it must not hold the restored bridge's first pass.
    if (passes.get(deviceId) === previous) passes.delete(deviceId);
  }
  if (!holdingLock || sessionOf(deviceId) !== session) return false;
  return syncDevice(deviceId);
}

/** The session this device is on right now, which is what a pass belongs to. */
const sessionOf = (deviceId) => contextFor(deviceId)?.session ?? null;

/** Run one pass and hold it where the next ask can find it. The promise never
 *  rejects: a pass that threw is a pass that did not finish, and the caller's
 *  question is only ever whether it got all the way through. */
function startPass(deviceId) {
  const turn = { session: sessionOf(deviceId), superseded: false, reading: false, done: null };
  turn.done = (async () => {
    try {
      return await orderedSync(deviceId, turn);
    } catch {
      return false;
    } finally {
      if (passes.get(deviceId) === turn) passes.delete(deviceId);
    }
  })();
  passes.set(deviceId, turn);
  return turn.done;
}

async function orderedSync(deviceId, turn) {
  const context = await greetedContext(deviceId, turn);
  if (!context) return false;
  await subscribeDevice(context);
  if (!context.active()) return false;
  await heldBeforeReading(context);
  if (!context.active()) return false;
  readingSessions.set(deviceId, turn.session);
  turn.reading = true;
  const passStartedAt = Date.now();
  const fence = pushFence();
  const view = await readLists(context, fence);
  if (!view || !context.active()) return false;
  const pass = await workspacesToRead(context, view);
  await readWorkspaces(context, pass, fence);
  await readProjectIssues(context, view);
  await evictRowsThatAreOver(context, view, fence);
  await dropWhatTheBoardStoppedNaming(context, view, passStartedAt, fence);
  if (!context.active()) return false;
  // The reader may have walked off while the pass read, and on a cold cache
  // the workspace they stand in had no row to resolve until the lists landed.
  await refollowRoute(deviceId);
  return true;
}

/** This device, once its bridge has said what it speaks. A bridge says which
 *  API major it answers in in its greeting, and until that has settled,
 *  asking it anything is asking for an answer in a shape this tab may not be
 *  able to read. Null where a reconnect landed under the wait. */
async function greetedContext(deviceId, turn) {
  const greeting = contextFor(deviceId)?.greeted;
  if (!(await settledWithin(greeting, GREETING_WAIT_MS))) return null;
  if (greeting !== contextFor(deviceId)?.greeted) return null;
  const context = syncContext(contextFor(deviceId), turn);
  return context?.active() ? context : null;
}

/** Whether this promise settled inside the wait — however it settled. False is
 *  "it is still out there", and the caller stands down rather than holding a
 *  turn nothing will ever end. */
function settledWithin(promise, waitMs) {
  if (!promise) return Promise.resolve(true);
  let timer = null;
  const settled = Promise.resolve(promise).then(() => true, () => true);
  return Promise.race([
    settled,
    new Promise((resolve) => {
      timer = setTimeout(() => resolve(false), waitMs);
    }),
  ]).finally(() => clearTimeout(timer));
}

/** Every workspace in the pass's order — less any a push has taken off the
 *  board since the lists were read, which would only be read back into
 *  records nothing names. One the push takes away while it is being read
 *  stops where it stands. */
async function readWorkspaces(context, pass, fence) {
  for (const entityId of pass.order) {
    if (!context.active()) return;
    if (removedSince(addressOf(context, entityId, "row"), fence)) continue;
    await syncWorkspace(whileOnBoard(context, entityId, fence), entityId, pass.rows.get(entityId), entityId === pass.routed);
  }
}

/** The context for one entity's records, stood down as well once a push takes
 *  the entity off the board after the fence: a read already out when it went
 *  writes nothing back under it, and nothing after that read is asked. Every
 *  write here is behind `active()`, which is what makes this reach them all. */
const whileOnBoard = (context, entityId, fence) => ({
  ...context,
  active: () => context.active() && !removedSince(addressOf(context, entityId, "row"), fence),
});

/** Step 1: the three lists, written as they land so the inbox paints before
 *  any workspace has been read. A device whose bridge does not serve
 *  workspaces still has a board. */
async function readLists(context, fence) {
  const [board, projects, workspaces] = await Promise.all([
    ask(context, "board.list", {}, "background"),
    ask(context, "project.list", {}, "background"),
    ask(context, "workspace.list", {}, "background"),
  ]);
  if (!board || !projects || !context.active()) return null;
  await writeUsageLimits(context.deviceId, board.usage_limits);
  const view = liveFeedSnapshot(board, projects, workspaces || { workspaces: [] }, context.deviceId);
  await writeLists(context, view, fence);
  return view;
}

async function writeLists(context, view, fence) {
  if (!context.active()) return;
  // Inside the feed's own transaction, so a push landing while it waits is
  // still seen: the rows a push wrote since the fence are not this read's
  // observation, and their own records paint over the board's older copy.
  await updateCachedFeed(addressOf(context, "", "feed"), () => withoutPushedRemovals(context, view, fence), {
    observedFeedRows: true,
    supersededFeedRow: (row) => Boolean(entityIdOf(row) && pushedSince(addressOf(context, entityIdOf(row), "row"), fence)),
  });
  for (const kind of ["projects", "workspaces"]) {
    if (!context.active()) return;
    if (!pushedSince(addressOf(context, "", kind), fence)) await writeSessionList(context, kind, view[kind]);
  }
  for (const row of view.items || []) {
    if (!context.active()) return;
    await writeListedRow(context, row, fence);
  }
}

/** One row as the board listed it — unless a push has written or removed it
 *  since the board was asked, which is the newer word. */
async function writeListedRow(context, row, fence) {
  const entityId = entityIdOf(row);
  if (!entityId || pushedSince(addressOf(context, entityId, "row"), fence)) return;
  await writeCached(addressOf(context, entityId, "row"), row);
  await writeSurfaces(context, entityId, row.agents);
}

/** The snapshot without the entities a push said left after it was asked for:
 *  written back, the board's older word would put their rows on the rail
 *  again. */
function withoutPushedRemovals(context, view, fence) {
  const gone = new Set();
  for (const field of BOARD_COLLECTIONS) {
    for (const row of view[field] || []) {
      const entityId = entityIdOf(row);
      if (entityId && pushedSince(addressOf(context, entityId, "row"), fence)?.removed) gone.add(entityId);
    }
  }
  return gone.size ? withoutEntities(view, gone) : view;
}

/**
 * The surface snapshots an agent digest carries.
 *
 * A surface — an agent's goal, its checklist — is an observation the live
 * session makes, so the digest carries it only while that session is up. The
 * record is what the rail paints for a session that has since died, which is
 * why it is kept beside the row rather than read back off it, and why it is
 * written under the generation that observed it: a restarted process must not
 * inherit the last one's goal.
 *
 * Written only where the snapshot moved. The record's own write time is how
 * long a pill lingers after its session goes (core/agentSurfacesModel.js), and
 * a rewrite on every unchanged push would hold that grace open for ever.
 */
async function writeSurfaces(context, entityId, agents) {
  for (const agent of agents || []) {
    if (!context.active()) return;
    await writeAgentSurfaces(context, entityId, agent);
  }
}

/** What the digest observed, as a fingerprint — or null for an agent with no
 *  live session, which has observed nothing this record could be about.
 *
 *  A live session carrying no surfaces HAS observed something: that there is
 *  nothing to show. That is written down as such, so a reader coming back is
 *  not painted a snapshot the session has since let go of. */
const observedSurfaces = (agent) => {
  const generation = surfaceSessionGeneration(agent?.surface_session_generation);
  if (!generation) return null;
  const observed = agent.surfaces ?? null;
  return { generation, fingerprint: surfacesFingerprint(observed, generation), observed };
};

const heldSurfaces = (held) => surfacesFingerprint(held?.surfaces, surfaceSessionGeneration(held?.generation));

async function writeAgentSurfaces(context, entityId, agent) {
  const seen = observedSurfaces(agent);
  if (!seen?.fingerprint) return;
  const address = surfacesCacheAddress({ deviceId: context.deviceId, entityId, agentId: agent.id });
  const held = (await readCached(address))?.value;
  if (!context.active() || seen.fingerprint === heldSurfaces(held)) return;
  await writeCached(address, surfacesRecord(seen.observed, seen.generation));
}

/** The conversation each project holds, as `project.list` names it. A project
 *  nobody has talked to yet holds none, and names nothing here.
 *
 *  Active whatever the inbox says about the row underneath it: a project's
 *  conversation is not work that finishes — there is no branch behind it to
 *  merge and no row to clear — and the project page offers it the moment the
 *  reader opens the project. Letting it age into Recent would leave that page
 *  blank on the first sync of a new session, and blank until somebody spoke:
 *  a Recent row is only read where the cache already holds its data, which on
 *  a session that has never synced is nothing at all. */
const projectConversationIds = (view) =>
  (view.projects || []).map((project) => project.entity_id || project.run_id).filter(Boolean);

/** Every project this device lists, by the id the wire carries. What the
 *  tracker's records are addressed by: an issue belongs to a project and never
 *  moves between projects, and a project is named by the machine it is on. */
const projectIds = (view) => (view.projects || []).map((project) => project.project_id || project.id).filter(Boolean);

/**
 * Step 2b: every project's issue list.
 *
 * Read on every pass for the same reason a project's conversation is
 * (`projectConversationIds` above): the Issues tab offers itself the moment the
 * reader opens a project, and a project is not a board row that ages into
 * Recent — it holds no work that finishes. Reading it only when routed there
 * would leave the tab blank on the first pass of a new session, and blank until
 * somebody filed something.
 *
 * The whole list, narrowed by nothing: the tab's filters are `issues.list`
 * params of their own (core/trackerFilters.js), and a record already narrowed
 * would be missing whatever the next filter is about to ask for.
 *
 * Behind the workspaces, never in front of them. An issue list is a project
 * surface and the inbox is the landing one, so nothing on screen waits on this.
 */
async function readProjectIssues(context, view) {
  for (const projectId of projectIds(view)) {
    if (!context.active()) return;
    await readIssues(context, projectId);
  }
}

/** One project's issues, and its columns the first time. The columns change
 *  with the project rather than with an issue, so they are asked for once and
 *  held; a bridge that refuses the verb leaves them empty and every board falls
 *  back to phase 1's five (core/trackerModel.js). A bridge that serves no
 *  tracker at all refuses both and writes nothing, which is a cold Issues tab
 *  and never an error the reader sees. */
async function readIssuesNow(context, projectId) {
  const answer = await ask(context, "issues.list", { project_id: projectId }, "background");
  if (!answer || !context.active()) return;
  const held = await readIssuesRecord(context.deviceId, projectId);
  const columns = held?.columns?.length
    ? held.columns
    : (await ask(context, "issues.columns", { project_id: projectId }, "background"))?.columns || [];
  if (!context.active()) return;
  await Promise.all([
    writeIssuesRecord(context.deviceId, projectId, issuesRecord(answer.issues, columns)),
    writeUserSession(context.deviceId, answer),
  ]);
}

/** One project's list read at a time on a session (#119). A push heard while
 *  it is out is read once after it, under the newest context that asked. */
const latestIssueReads = new Map();
const issueReadGenerations = new Map();
function issueReadGeneration(deviceId) {
  const session = sessionOf(deviceId);
  const requestScope = restoredScopes.get(deviceId)?.session === session
    ? restoredScopes.get(deviceId).scope : null;
  const held = issueReadGenerations.get(deviceId);
  if (held?.session === session && held?.requestScope === requestScope) return held;
  const generation = { session, requestScope };
  issueReadGenerations.set(deviceId, generation);
  return generation;
}
const issueReads = trailingRead((key) => {
  const { context, projectId } = latestIssueReads.get(key);
  return readIssuesNow(context, projectId);
}, { generationOf: (key) => issueReadGeneration(latestIssueReads.get(key).context.deviceId) });

function readIssues(context, projectId) {
  const key = JSON.stringify([context.deviceId, projectId]);
  const generation = issueReadGeneration(context.deviceId);
  latestIssueReads.set(key, {
    context: { ...context, active: () => context.active() && issueReadGeneration(context.deviceId) === generation },
    projectId,
  });
  return issueReads(key);
}

/**
 * Step 2: which workspaces this pass reads, and in what order.
 *
 * The active set is the inbox's own partition and every project's own
 * conversation, plus the Recent rows that still hold data — and a Recent row
 * is aged out before it is asked that question, so "still holds data" means
 * "was written to inside the TTL", which is the rule. The routed workspace
 * leads, whatever the inbox order says.
 */
async function workspacesToRead(context, view) {
  const items = view.items || [];
  const rows = new Map();
  // `items` is the inbox, so it omits a run after every agent on it is
  // unwatched. The routed workspace still owns that run and its thread; the
  // board's `runs` collection carries the roster needed to sync it.
  for (const run of view.runs || []) {
    const entityId = entityIdOf(run);
    if (entityId) rows.set(entityId, run);
  }
  for (const row of items) {
    const entityId = entityIdOf(row);
    if (entityId) rows.set(entityId, row);
  }
  const active = [...new Set([...cacheableEntityIds({ items }), ...projectConversationIds(view)])];
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
    await expireWorkspaceData(context.deviceId, entityId, Date.now(), context.active);
    const left = await cachedAddresses({ deviceId: context.deviceId, entityId });
    if (left.some((address) => isWorkspaceDataKind(address.kind))) kept.push(entityId);
  }
  return kept;
}

/** Step 4: a row the board still lists but whose work is over keeps nothing.
 *  The push that said so evicts it as it lands — but a tab that was not open
 *  to hear it boots to a board that already says the row is over, and the rule
 *  has to hold there too. */
async function evictRowsThatAreOver(context, view, fence) {
  for (const row of view.items || []) {
    if (!context.active()) return;
    const entityId = entityIdOf(row);
    if (!entityId || !isFinishedState(row.state)) continue;
    // A push since the board was read is the row's newer word, and its own
    // applier evicted the data if that word was "over".
    if (pushedSince(addressOf(context, entityId, "row"), fence)) continue;
    await evictWorkspaceData(context.deviceId, entityId, context.active);
  }
}

/** Step 4, the other half: an entity the board has stopped naming altogether
 *  is gone — the row is the board's to list and the board's to remove, so the
 *  row goes with the data. Another device's records are another device's
 *  business, and an entity a push named after the board was read is one the
 *  board had not heard of yet. */
async function dropWhatTheBoardStoppedNaming(context, view, passStartedAt, fence) {
  const named = entitiesTheBoardNames(view);
  const visible = new Set((view.items || []).map(entityIdOf));
  const hiddenRuns = new Set((view.runs || []).map(entityIdOf).filter((id) => id && !visible.has(id)));
  const pushedRow = (entityId) => pushedSince(addressOf(context, entityId, "row"), fence);
  for (const cachedId of await cachedEntityIds(context.deviceId)) {
    if (!context.active()) return;
    if (!named.has(cachedId)) {
      if (!pushedRow(cachedId)) await dropUnnamedEntity(context, cachedId);
    } else if (hiddenRuns.has(cachedId)) {
      await dropOlderHiddenRow(context, cachedId, passStartedAt);
    }
  }
}

/** Keep the live conversation's records, but remove an older inbox row:
 *  `taskFeed` appends standalone row records to the board's `items`, and an
 *  unwatched run is deliberately absent from that list. A state push after
 *  this pass began owns a newer row and must not be erased. */
async function dropOlderHiddenRow(context, entityId, passStartedAt) {
  const address = addressOf(context, entityId, "row");
  const row = await readCached(address);
  if (!context.active()) return;
  if (row && row.at < passStartedAt) await deleteCached([address]);
}

const entitiesTheBoardNames = (view) => {
  const named = new Set();
  for (const row of [...(view.items || []), ...(view.runs || [])]) {
    const entityId = entityIdOf(row);
    if (entityId) named.add(entityId);
  }
  // A project is not a work row, but its issues are cached under its id
  // (core/trackerCache.js) — so a project the lists still name is still named
  // here, and one that has gone takes its issues with it.
  for (const projectId of projectIds(view)) named.add(projectId);
  return named;
};

/**
 * Everything an unnamed entity holds — except the issue surface's records.
 *
 * Issues LEFT the board (core/issueCache.js): no `board.list` names one, no
 * pass fills one, and nothing but the issue surface itself ever writes one. So
 * "the board stopped naming it" is not news about an issue — it is the
 * standing state of every issue there is, and a pass that read it as a
 * departure would take the records back out on every boot, reconnect and tab
 * return. The surface's mount-from-cache frame would then be a frame nobody
 * ever sees.
 *
 * Only the kind is exempt, and only from this rule: everything else the entity
 * holds still goes, a Done or a Delete still takes an issue's records with the
 * rest (core/cacheLifetime.js), and the 72 h sweep still ages them out.
 */
async function dropUnnamedEntity(context, entityId) {
  const issueRecords = await cachedSubKeys(context.deviceId, entityId, ISSUE_RECORD_KIND);
  if (!context.active()) return;
  if (!issueRecords.length) {
    await evictEntity(context.deviceId, entityId);
    return;
  }
  const held = await cachedAddresses({ deviceId: context.deviceId, entityId });
  if (!context.active()) return;
  await deleteCached(held.filter((address) => address.kind !== ISSUE_RECORD_KIND));
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
  const log = scope ? await syncCommits(context, entityId, scope, priority) : null;
  await syncThreads(context, entityId, row, priority);
  if (!scope) return;
  await syncPatches(context, entityId, scope, unpushedCommits(log), priority);
  await syncWorkingDiff(context, entityId, row, priority);
}

/** The status, asked conditionally: the `status_key` the cache holds is the
 *  same key `git.status` answers with, so holding it is proof there is nothing
 *  to fetch and the answer says only that. */
async function syncStatus(context, entityId, scope, priority) {
  const held = await heldValue(context, entityId, "status");
  const params = held?.status_key ? { ...scope, if_status_key: held.status_key } : scope;
  await readWhole(context, addressOf(context, entityId, "status"), { method: "git.status", params, priority });
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
    await readWhole(
      context,
      addressOf(context, entityId, "tree", path),
      { method: "fs.tree", params: { ...scope, path }, priority },
      (listing) => ({ path: listing.path || "", entries: listing.entries || [] }),
    );
  }
}

async function syncTerminals(context, entityId, row, priority) {
  const scope = terminalScopeOf(row);
  if (!scope) return;
  await readWhole(
    context,
    addressOf(context, entityId, "terminals"),
    { method: "term.list", params: scope, priority },
    (answer) => ({ tabs: answer.terminals || [] }),
  );
}

/** The two commit lists, answered whole. The commit record is handed back: the
 *  patches behind its unpushed commits are read later in the pass, once the
 *  conversations are in. */
async function syncCommits(context, entityId, scope, priority) {
  await syncLog(context, entityId, scope, priority);
  await syncUnpushed(context, entityId, scope, priority);
  return heldValue(context, entityId, "log");
}

/** The commits this checkout has that its push target does not, newest first.
 *  `git.unpushed` answers one aggregate diff and no commit list, so the list
 *  is the log's own: the bridge marks every commit ahead of the base. */
const unpushedCommits = (log) => (log?.commits || []).filter((commit) => commit.ahead_of_base);

/** The commit list, read forward from the newest hash the cache holds — or the
 *  latest 20 when it holds none. */
async function syncLog(context, entityId, scope, priority) {
  const held = await heldValue(context, entityId, "log");
  const params = held?.newest ? { ...scope, since: held.newest } : { ...scope, limit: LATEST_COMMITS };
  const answer = await ask(context, "git.log", params, priority);
  if (!answer || !context.active()) return;
  await mergeCached(addressOf(context, entityId, "log"), (current) =>
    context.active() ? mergedLog(current, answer) : null);
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
  const arrived = new Map(arriving.map((commit) => [commit.hash, commit]));
  const kept = new Set(previous.map((commit) => commit.hash));
  // An answer carrying nothing the record does not already hold leaves the
  // record's own ORDER alone. Another writer reached it first with more of
  // this history than this answer walked, and putting these commits back in
  // front of it would stand an older commit at the head of the list. What the
  // answer says about each commit is still the newer word: a hash is fixed,
  // but `ahead_of_base` is true until the branch is pushed and false after,
  // and a record keeping its own copy of that would go on calling a published
  // commit unpushed with no read left to correct it.
  const commits = arriving.every((commit) => kept.has(commit.hash))
    ? previous.map((commit) => arrived.get(commit.hash) || commit)
    : [...arriving, ...previous.filter((commit) => !arrived.has(commit.hash))];
  return { ...rest, commits, more: moreAfter(held, answer, previous), newest: newestAfter(held, answer, commits) };
}

/** Whether history reaches back past the last commit the record holds.
 *
 *  A cursored read walks forward from the record's newest hash to HEAD, so
 *  what it says about there being more is about the window it walked and not
 *  about the end of a list it never reached — a caught-up checkout answers no
 *  commits and `more: false`. Where the record's own commits are still the
 *  tail of the list, the record's own answer stands; where they are gone — a
 *  reset, or a pushed window that does not reach them — the answer's does. */
const moreAfter = (held, answer, previous) => (previous.length ? (held.more ?? answer.more) : answer.more);

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

/** The cursor the record reads forward from next: the newest commit the record
 *  is left holding, else what the answer named, else the cursor it already
 *  had.
 *
 *  The record's own head leads because a walk is answered newest first from
 *  HEAD — so the two agree, except where another writer got to the record
 *  with more of the history than this answer walked. There the record is what
 *  the next read must read forward from.
 *
 *  A reset takes the last of the three with it. The hash this cache was
 *  reading from is one the checkout no longer has, so keeping it would have
 *  the next read ask after it again and be answered `reset` again, for ever.
 *  With no cursor the next read asks for the latest commits, which is what a
 *  cache that knows nothing of a history asks for. */
const newestAfter = (held, answer, commits) =>
  commits[0]?.hash || answer.newest || (answer.reset ? null : held?.newest || null);

/** The unpushed commits, without the patch that rides with them: the Records
 *  table holds the base, the commit list and the diff key, and the patch
 *  behind each commit is its own record under its own cap. */
const unpushedRecord = (answer) => {
  const record = { ...answer };
  delete record.patch;
  return record;
};

async function syncUnpushed(context, entityId, scope, priority) {
  // Without the aggregate patch: this record never held it — `unpushedRecord`
  // deletes it on arrival — and asking for it anyway put most of a megabyte on
  // the wire for every cold pass. A reader opening the review asks for the
  // body itself, from the pane, uncapped.
  return readWhole(
    context,
    addressOf(context, entityId, "unpushed"),
    { method: "git.unpushed", params: { ...scope, patch: false }, priority },
    unpushedRecord,
  );
}

/** The patches behind the unpushed commits: the first twenty, and only the
 *  ones not already held.
 *
 *  Read under the cap, which is what a cache asks under: a commit over it is
 *  answered with the file headers, and those are kept. Holding which files
 *  moved is what stops the next pass asking after the commit again — and a
 *  reader opening it is what asks for the patch itself, uncapped, from the
 *  pane. */
async function syncPatches(context, entityId, scope, commits, priority) {
  const hashes = (commits || [])
    .map((commit) => commit.hash)
    .filter(Boolean)
    .slice(0, UNPUSHED_COMMITS_MAX);
  const held = await cachedSubKeys(context.deviceId, entityId, PATCH_RECORD_KIND);
  if (!context.active()) return;
  await dropStalePatches(context, entityId, held, new Set(hashes));
  for (const hash of hashes) {
    if (held.includes(hash) || !context.active()) continue;
    const answer = await ask(context, "git.show", { ...scope, hash, max_bytes: COMMIT_PATCH_MAX_BYTES }, priority);
    if (!answer || !context.active() || answer.truncated || !withinBytes(answer.patch, COMMIT_PATCH_MAX_BYTES)) continue;
    await writeCached(addressOf(context, entityId, PATCH_RECORD_KIND, hash), answer);
  }
}

/** A patch for a commit that is no longer unpushed has been published: it is
 *  in the log like every other commit, and nobody is reviewing it here. */
async function dropStalePatches(context, entityId, held, wanted) {
  if (!context.active()) return;
  const stale = held.filter((hash) => !wanted.has(hash));
  if (stale.length) await deleteCached(stale.map((hash) => addressOf(context, entityId, PATCH_RECORD_KIND, hash)));
}

/** Which verb answers this row's working-tree diff, and what to name the read
 *  so a view asking for the same body shares it.
 *
 *  `patch: false` asks for the shape without the hunks — the stat, the
 *  per-file rows and the key that names the body. A read that names it is a
 *  different read from one that does not, so it rides the key too: two callers
 *  wanting different things must never be folded into one answer. */
function diffRead(row, held, { patch = true } = {}) {
  const runId = row.run_id;
  const method = runId ? "run.diff" : "worktree.diff";
  const repository = runId ? `run:${runId}` : `worktree:${row.project_id}:${row.worktree_id}`;
  const base = runId ? { run_id: runId } : { project_id: row.project_id, worktree_id: row.worktree_id };
  const asked = patch ? base : { ...base, patch: false };
  return { method, repository, params: held?.diff_key ? { ...asked, if_diff_key: held.diff_key } : asked };
}

/**
 * The working-tree diff, as a PASS asks for it: the stat, the per-file rows
 * and the key, and none of the hunks.
 *
 * The body is the largest thing a workspace holds — three quarters of a
 * megabyte on a checkout with real work in it — and a pass that pulled one per
 * workspace put megabytes on the wire before the reader had opened anything.
 * Over a phone's relayed path that is the first ten seconds of every session
 * spent on hunks nobody is looking at, and it is what the reader's own
 * connection was competing with.
 *
 * Nothing goes blank for it: the Changes surface reads the wire itself when it
 * mounts over a record the sync layer wrote (`core/changesReview.js`,
 * `bodyOnly`), which it already did before this — the pass's body was being
 * fetched twice over.
 */
async function syncWorkingDiff(context, entityId, row, priority) {
  if (await readCached(addressOf(context, entityId, "diff"))) return;
  await pullWorkingDiff(context, entityId, row, priority, { patch: false });
}

async function pullWorkingDiff(context, entityId, row, priority, { patch = true } = {}) {
  const address = addressOf(context, entityId, "diff");
  const held = (await readCached(address))?.value;
  const { method, repository, params } = diffRead(row, held, { patch });
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
  await mergeCached(address, (current) => context.active() ? diffRecord(current, diff, row) : null);
}

/** The diff record after a new body, wherever the body came from. The body
 *  replaces what was held; which project the diff belongs to is nothing the
 *  wire knows about and stays where it was put. A body in hand is the end of
 *  whatever staleness put the record here. */
const diffRecord = (held, diff, row) => {
  const oversized = diff.truncated || !withinBytes(diff.patch, WORKING_DIFF_MAX_BYTES);
  const record = { ...held, ...diff, stale: Boolean(oversized), projectId: row?.project_id || held?.projectId || null };
  if (oversized) delete record.patch; // #94: body pages are deferred to #95.
  return record;
};

/** The record after a push that HAD a diff and could not send it: too big for
 *  the cap. The body held is the last one anybody saw, so it stays on screen —
 *  marked, so the surface that opens it reads the real one once rather than
 *  showing yesterday's tree for ever. */
const staleDiffRecord = (held) => (!held || held.stale ? null : { ...held, stale: true });

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
  await syncThreadWindow({
    deviceId: context.deviceId,
    call: context.call,
    active: context.active,
    entityId,
    agentId: agent.id,
    conversationId: agent.conversation_id,
    priority,
  });
}

// ─── Step 0: the three subscriptions ─────────────────────────────────────────
//
// `s-inbox` carries every workspace's state and conversation in realtime —
// that is the inbox, and it is what the reader is looking at whatever page
// they are on. `s-background` carries every workspace's git, files and shells
// on a 30 s cooldown. `s-active` carries the same kinds for the one workspace
// the reader is standing in, in realtime, and is re-issued when they move.

const subscriptionShape = (deviceId) => ({
  deviceId,
  refresh: NOTHING,
  // A push carries bodies, so a hidden tab writing them is a hidden tab that
  // paints instantly when it comes back. Nothing here is a read to defer.
  pausesWhileHidden: false,
  onChanges: (items) => void applyChanges(items, deviceId),
});

/** The three watchers, and the one that follows the reader — taken out before
 *  the pass reads (`heldBeforeReading` says how long it waits for them).
 *
 *  The routed workspace is resolved again at the end of the pass rather than
 *  carried from here: a pass is six or eight reads per workspace long and the
 *  reader walks off mid-way through it. Standing the active subscription up on
 *  where they were when it started would leave the workspace on screen with no
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
        kinds: inboxPushKinds(deviceId),
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

/**
 * Held on the bridge before the pass reads a thing (#142) — where the reader
 * has something cached to look at meanwhile. A device the cache holds nothing
 * for reads at once: its reader sees nothing until it does, and a subscribe
 * can take as long as any receipted call.
 *
 * The wait is bounded like the greeting and saves a pass; it does not close
 * the gap. A subscription that lands after the reads were asked is what does
 * that, whichever way the reads came to be first (`subscriptionLanded`).
 */
async function heldBeforeReading(context) {
  if (!(await readCached(addressOf(context, "", "feed")))) return;
  await settledWithin(subscriptionsSettledFor(context.deviceId), GREETING_WAIT_MS);
}

/** The subscriptions a pass stands on. The active one is a realtime copy of
 *  part of the background one, so its landing late opens no gap. */
const PASS_SUBSCRIPTIONS = new Set(["s-inbox", "s-background"]);

/** Per device, the session a pass has started reading: the reads it asks are
 *  covered by whatever the bridge held at the time, and no more. */
const readingSessions = new Map();

/** The bridge now holds a subscription a pass on this session read without:
 *  whatever changed between that read and this answer was recorded for
 *  nobody. Read again once the pass that is out, if one is, has finished — or
 *  not at all where that pass has yet to ask for anything, since everything
 *  it reads is read after this. */
function subscriptionLanded(deviceId, subscriptionId) {
  if (!holdingLock || !PASS_SUBSCRIPTIONS.has(subscriptionId)) return;
  const session = sessionOf(deviceId);
  if (session === null || readingSessions.get(deviceId) !== session) return;
  const running = passes.get(deviceId);
  if (running && !running.reading) return;
  void readAgainAfter(deviceId, session);
}

async function readAgainAfter(deviceId, session) {
  await passes.get(deviceId)?.done;
  if (holdingLock && sessionOf(deviceId) === session) await syncDevice(deviceId);
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
 *  A device whose pass has not taken its subscriptions out yet is not here to
 *  be told, and does not need to be: that pass resolves the route for itself
 *  when it does, and again when it ends. */
export function routeChanged() {
  if (!holdingLock) return;
  for (const deviceId of [...subscriptions.keys()]) void refollowRoute(deviceId);
}

async function refollowRoute(deviceId) {
  if (!contextFor(deviceId) || !subscriptions.has(deviceId)) return;
  // Off the records rather than off the `feed` this pass wrote: the workspace
  // the reader is standing in may have been made since (core/cachedRows.js).
  const entityId = await cachedRouteEntityId(deviceId, App.route);
  if (!subscriptions.has(deviceId)) return;
  followRoutedEntity(deviceId, entityId);
}

// ─── Applying a push ─────────────────────────────────────────────────────────

const BOARD_ENTITY = "board";

const validSession = (record) => Number.isSafeInteger(record?.session_started_ms)
  && Number.isSafeInteger(record?.last_activity_ms)
  && record.session_started_ms <= record.last_activity_ms;

async function writeSessionList(context, kind, incoming) {
  await replaceSessionList(addressOf(context, "", kind), kind, incoming);
}

/** A record as a push carries it, marked so a read that was already out when
 *  it landed does not write its older answer over it. */
function writePushed(address, value) {
  notePush(address);
  return writeCached(address, value);
}

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
  const scoped = whileOnBoard(context, entityId, pushFence());
  for (const [field, apply] of APPLIERS) {
    if (!scoped.active()) return;
    if (item[field]) await apply(scoped, entityId, item[field]);
  }
}

/** The board item: which entities left the board, and the two lists when they
 *  moved. An entity that finished, was deleted, or was cleared away appears in
 *  `removed`, and everything it had goes at once — its data and its row. */
async function applyBoard(context, state) {
  // The harnesses out of usage there (#58): the pushed reading replaces the
  // device's record, and mounted surfaces repaint from its cache announcement.
  await writeUsageLimits(context.deviceId, state.usage_limits);
  if (!context.active()) return;
  const removed = (state.removed || []).map((entityId) => String(entityId)).filter(Boolean);
  if (removed.length) await dropRemovedRows(context, removed);
  if (!context.active()) return;
  if (state.projects) {
    notePush(addressOf(context, "", "projects"));
    await writeSessionList(context, "projects", state.projects.map((project) => stampProject(project, context.deviceId)));
  }
  if (!context.active()) return;
  if (state.workspaces) {
    // A row that names its own verdict — a list a git flush re-sent, with the
    // summary that flush re-read — is the fresh word. A row that names none
    // (a list that moved because a workspace came or went) keeps the verdict
    // the cache holds rather than losing its Done until the next whole read.
    const summaries = workspaceSummaries(await heldValue(context, "", "workspaces"));
    if (!context.active()) return;
    notePush(addressOf(context, "", "workspaces"));
    await writeSessionList(context, "workspaces", state.workspaces.map((workspace) =>
      stampWorkspace(workspace, context.deviceId, workspace.work_summary === undefined ? summaries : [])));
  }
}

/** The `feed` record's own collections: the board's five. The project and
 *  workspace lists ride the same item and are written whole. */
const BOARD_COLLECTIONS = FEED_COLLECTIONS.filter((field) => field !== "projects" && field !== "workspaces");

/**
 * The rows the board item says left, out of both halves of the rail.
 *
 * The rail is the `feed` record's list with each row's own record laid over it
 * (core/taskFeed.js), so a row survives until BOTH are gone. A row whose work
 * finished rides its own `state` item and is filtered by what that says; a row
 * that was DELETED — a branch removed from another machine, a workspace thrown
 * away — has no state to report and is named here and nowhere else. Dropping
 * only its data would leave the rail painting a row whose Done and Clear act
 * on something that is not there.
 *
 * The list first, then the entities under it: every write is announced and the
 * rail re-reads on the announcement, so evicting first would send it to a feed
 * that still names the row and paint it again between the two halves of its
 * own removal.
 */
async function dropRemovedRows(context, removed) {
  for (const entityId of removed) notePush(addressOf(context, entityId, "row"), { removed: true });
  const gone = new Set(removed);
  const record = await readCached(addressOf(context, "", "feed"));
  if (!context.active()) return;
  if (record) await writeCached(addressOf(context, "", "feed"), withoutEntities(record.value, gone));
  for (const entityId of removed) {
    if (!context.active()) return;
    await evictEntity(context.deviceId, entityId);
  }
}

/** One device's snapshot with those entities taken out of every collection the
 *  board fills. A row naming no entity is nobody's departure and stays. */
function withoutEntities(view, gone) {
  const pruned = { ...view };
  for (const field of BOARD_COLLECTIONS) {
    pruned[field] = (view[field] || []).filter((row) => !gone.has(entityIdOf(row)));
  }
  return pruned;
}

/** Whether a `state` item is a feed row at all.
 *
 *  A legacy issue left the board, so the bridge has no row to push for one and
 *  answers with the three-field digest it always did: a lifecycle word, an
 *  agent COUNT and an attention reason. Nothing cache-first reads that, and
 *  written where a row belongs it is a work item whose agents are a number —
 *  which every reader of the record then tries to walk. */
const isFeedRow = (state) => typeof state?.kind === "string" && state.kind !== "";

/** `state`: the feed row exactly as `board.list` carries it. A row whose work
 *  is over takes the workspace's data with it — nobody is coming back to it. */
async function applyState(context, entityId, state) {
  if (!isFeedRow(state)) return;
  await writePushed(addressOf(context, entityId, "row"), stampRow(state, context.deviceId));
  if (!context.active()) return;
  await writeSurfaces(context, entityId, state.agents);
  if (!context.active()) return;
  if (isFinishedState(state.state)) await evictWorkspaceData(context.deviceId, entityId, context.active);
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

/** A thread tip carries the summaries changed by its message. Project agent
 * tips update only the project; workspace tips update both owning records. */
async function applySessionTip(context, tip) {
  for (const [kind, id, session] of [
    ["workspaces", tip.workspace_id, tip.workspace_session],
    ["projects", tip.project_id, tip.project_session],
  ]) {
    if (!context.active()) return;
    if (!id || !validSession(session)) continue;
    await updateSessionSummary(addressOf(context, "", kind), kind, id, session);
  }
}

async function applyThreadTip(context, entityId, tip) {
  const sub = tipKey(tip);
  if (!sub) return;
  await applySessionTip(context, tip);
  if (!context.active()) return;
  const address = addressOf(context, entityId, THREAD_RECORD_KIND, sub);
  const held = (await readCached(address))?.value;
  if (!context.active() || !tipIsNews(tip, held)) return;
  // A burst past the push cap arrives as a tip with no items, a conversation
  // nothing is held for has nothing to append to, and a tip that starts past
  // the cursor would append over a gap. All three are one cursored page, which
  // is the read this layer would have made anyway.
  const items = held && tipRunsOnFromRecord(tip, held) ? (tip.items || []) : [];
  if (!items.length) {
    await syncThread(context, entityId, { id: tip.agent_id, conversation_id: tip.conversation_id }, "background");
    return;
  }
  await mergeCached(address, (current) => context.active() ? threadWindow(current, {
    items, thread_total: tip.thread_total,
  }) : null);
}

/** `git`: the shapes ride the push, so nothing is asked for them. Two things
 *  never ride one — the patch behind a commit, and a working-tree diff too big
 *  to send — and those are pulled here. */
async function applyGit(context, entityId, git) {
  const row = await heldValue(context, entityId, "row");
  if (!context.active()) return;
  if (git.status) await writePushed(addressOf(context, entityId, "status"), git.status);
  if (!context.active()) return;
  if (git.log) {
    await mergeCached(addressOf(context, entityId, "log"), (current) =>
      context.active() ? windowedLog(current, git.log) : null);
  }
  if (!context.active()) return;
  if (git.unpushed) await writePushed(addressOf(context, entityId, "unpushed"), unpushedRecord(git.unpushed));
  if (!context.active()) return;
  if (git.diff) await writePushedDiff(context, entityId, git.diff, row);
  if (!context.active()) return;
  await pullWhatTheGitItemCouldNotCarry(context, entityId, git, row);
}

async function writePushedDiff(context, entityId, diff, row) {
  const address = addressOf(context, entityId, "diff");
  await mergeCached(address, (current) => context.active() ? diffRecord(current, diff, row) : null);
}

async function pullWhatTheGitItemCouldNotCarry(context, entityId, git, row) {
  const scope = gitScopeOf(row);
  if (!scope) return;
  if (git.log) {
    const log = await heldValue(context, entityId, "log");
    if (!context.active()) return;
    await syncPatches(context, entityId, scope, unpushedCommits(log), "background");
  }
  if (!context.active()) return;
  if (git.diff !== null) return;
  // A null diff is one the bridge had and could not send. It is only worth a
  // round trip for the workspace on screen; the rest are marked, and read it
  // when a reader opens them.
  if (subscriptions.get(context.deviceId)?.activeId === entityId) {
    await pullWorkingDiff(context, entityId, row, "foreground");
    return;
  }
  await mergeCached(addressOf(context, entityId, "diff"), (current) =>
    context.active() ? staleDiffRecord(current) : null);
}

/** `files`: the root listing rides the push. The deeper ones the reader walked
 *  into are re-listed where a changed path sits in them — and all of them when
 *  the list was truncated, because a truncated list means "refetch the tree",
 *  not "these paths". */
async function applyFiles(context, entityId, files) {
  if (files.root) {
    await writePushed(
      addressOf(context, entityId, "tree", files.root.path || ""),
      { path: files.root.path || "", entries: files.root.entries || [] },
    );
  }
  if (!context.active()) return;
  const scope = gitScopeOf(await heldValue(context, entityId, "row"));
  if (!context.active() || !scope) return;
  const walked = await cachedSubKeys(context.deviceId, entityId, "tree");
  const stale = files.truncated ? walked : dirsOf(files.paths).filter((dir) => walked.includes(dir));
  await listTrees(context, entityId, scope, new Set(files.root ? stale : ["", ...stale]), "background");
  await rereadHeldFiles(context, entityId, scope, files);
}

/** A file body the reader has open — or had open recently enough for the cache
 *  to still hold it — is stale the moment a push names its path. It is the one
 *  thing a `files` item never carries, so it is read: a bounded pull of what
 *  the reader is already looking at, and never of a file nobody has opened.
 *
 *  A truncated list says "the tree moved" rather than which paths did, so
 *  every held body is re-read. There are at most `RECENT_FILES` of them. */
async function rereadHeldFiles(context, entityId, scope, files) {
  const held = await cachedSubKeys(context.deviceId, entityId, FILE_RECORD_KIND);
  const named = new Set(files.paths || []);
  const stale = files.truncated ? held : held.filter((path) => named.has(path));
  for (const path of stale) {
    if (!context.active()) return;
    await rereadFile(context, entityId, scope, path);
  }
}

async function rereadFile(context, entityId, scope, path) {
  const address = addressOf(context, entityId, FILE_RECORD_KIND, path);
  const openedAt = (await readCached(address))?.value?.openedAt;
  const file = await ask(context, "fs.read", { ...scope, path }, "background");
  if (!context.active()) return;
  // A read that answered nothing is a file that moved out from under the
  // reader, or a machine that stopped answering. The body held is the last one
  // anybody saw; a delete here would blank an open preview on a hiccup.
  if (!file) return;
  // A body the cache may not keep — grown past the cap, or answered truncated —
  // takes the record with it. The rule is about what may be STORED; the record
  // is of a file that has since moved, and leaving it would hand the reader the
  // body from before the change on their next open, with no round trip and
  // nothing saying so.
  const kept = await cacheFileBody({ deviceId: context.deviceId, entityId, path, file, openedAt });
  if (!kept) await deleteCached([addressOf(context, entityId, FILE_RECORD_KIND, path)]);
}

const applyTerminals = (context, entityId, terminals) =>
  writePushed(addressOf(context, entityId, "terminals"), { tabs: terminals.tabs || [] });

/** `issues`: which issues of this project moved. Content-free beyond the ids —
 *  and dropped altogether past 200 of them — so there is one answer either
 *  way, which is to read the project's list again. The entity here is a
 *  PROJECT, not a workspace: every other applier below is handed a board row's
 *  entity, and this one is handed the project the issues belong to. Not
 *  awaited: a read folded behind one already out settles only after the read
 *  that follows it, and the rest of the flush has nothing to wait for. */
const applyIssues = (context, projectId) => {
  void readIssues(context, projectId);
};

/** One writer per kind, in the order a reader would want them applied: what
 *  the row says, what was said in it, then the surfaces under it. */
const APPLIERS = [
  ["state", applyState],
  ["thread", applyThreadItem],
  ["issues", applyIssues],
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
  // Queued, but not for ever: a holder that has been frozen where it stands
  // cannot do the job and cannot hand the lock back (see LOCK_WAIT_MS).
  lockWait = setTimeout(takeLock, LOCK_WAIT_MS);
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
  clearTimeout(lockWait);
  lockWait = null;
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
 *  row are one pass. A pass that did not finish takes the mark off again: a
 *  session left marked on a stalled `board.list` would spend its whole life
 *  hearing only what changed after it and never reading what it missed.
 *  Unmarked, the next thing the device announces asks again. */
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
  stopLandingWatch = onSubscriptionHeld(subscriptionLanded);
  if (typeof document !== "undefined" && !visibilityWired) {
    document.addEventListener("visibilitychange", onVisibilityChange);
    visibilityWired = true;
  }
  acquireLock();
}

export function stopCacheSync() {
  if (stopDeviceWatch) stopDeviceWatch();
  stopDeviceWatch = null;
  if (stopLandingWatch) stopLandingWatch();
  stopLandingWatch = null;
  for (const held of subscriptions.values()) {
    held.inbox.dispose();
    held.background.dispose();
    held.active?.dispose();
  }
  subscriptions.clear();
  syncedSessions.clear();
  restoredScopes.clear();
  readingSessions.clear();
  issueReadGenerations.clear();
  forgetPushes();
  // Whatever is still out stands down where it stands: its writes are all
  // behind `active()`, which this takes away with the lock.
  for (const turn of passes.values()) turn.superseded = true;
  passes.clear();
  clearTimeout(lockWait);
  lockWait = null;
  if (visibilityWired && typeof document !== "undefined") {
    document.removeEventListener("visibilitychange", onVisibilityChange);
    visibilityWired = false;
  }
  holdingLock = false;
  if (releaseLock) releaseLock();
  releaseLock = null;
}
