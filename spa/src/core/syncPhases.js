// A pass gives the open conversation and checkout the wire, then fills the
// inbox, and only then warms checkout records nobody is looking at. Route
// intent is resolved between reads, never captured for the entire pass.
import { discoveredThreadKey } from "./syncSuccessors.js";
import { routedEntityId } from "./inbox.js";
import { visibleTreePaths, visibleWorkingDiff, visibleSyncThread, visibleSyncReads } from "./syncVisibleScopes.js";
import { cachedSubKeys } from "./localCache.js";
import { requestPriorityFields } from "./readRequests.js";
import { drainBackgroundStreams } from "./syncBackgroundStreams.js";

const readsGroup = (state, group) => !state.readGroups || state.readGroups.has(group);
const entityReadsGroup = (state, group, entityId) => readsGroup(state, group) || state.discovery?.[group]?.has(entityId);
const readsThread = (state, entityId, agent) => readsGroup(state, "threads")
  || state.discovery?.threads?.has(discoveredThreadKey(entityId, agent));
const keyOf = (entityId, kind, sub = "") => JSON.stringify([entityId, kind, sub]);
const projectIds = (view) => (view.projects || []).map((project) => project.project_id || project.id).filter(Boolean);

function agentConversation(rows, agentId) {
  for (const [entityId, row] of rows) {
    const agent = (row.agents || []).find((item) => item.id === agentId);
    if (agent) return { entityId, agent };
  }
  return null;
}

function conversationFor(route, entityId, rows, deviceId) {
  const visible = visibleSyncThread(deviceId);
  if (visible) {
    const agent = rows.get(visible.entityId)?.agents?.find((agent) => agent.id === visible.agentId);
    if (agent) return { entityId: visible.entityId, agent };
  }
  if (route.agent) return agentConversation(rows, route.agent);
  const agent = rows.get(entityId)?.agents?.[0];
  return agent ? { entityId, agent } : null;
}

function scopeJobs(state, entityId, priority, paths) {
  const row = state.pass.rows.get(entityId);
  if (!row || !entityReadsGroup(state, "checkout", entityId)) return [];
  const scope = state.readers.scope(row);
  const job = (kind, sub, read) => ({ key: keyOf(entityId, kind, sub), read: () => read(state.entityContext(entityId), entityId, row, priority) });
  const jobs = [];
  if (scope) {
    jobs.push(job("status", "", state.readers.status));
    for (const path of paths) jobs.push(job("tree", path, (context, id, row, priority) => state.readers.tree(context, id, row, path, priority)));
  }
  jobs.push(job("terminals", "", state.readers.terminals));
  if (scope) {
    jobs.push(job("log", "", state.readers.log), job("unpushed", "", state.readers.unpushed));
  }
  return jobs;
}

const threadJob = (state, entityId, agent, priority) => ({
  key: keyOf(entityId, "thread", agent.conversation_id || agent.id),
  read: () => state.readers.thread(state.entityContext(entityId), entityId, agent, priority),
});

const SOURCE_ORDER = ["status", "tree", "terminals", "log", "unpushed", "diff"];
function sourceJobs(state, diffOnly = false) {
  return visibleSyncReads(state.context.deviceId)
    .filter(({ key }) => entityReadsGroup(state, "checkout", key.entityId))
    .filter(({ key }) => !state.pass.rows.has(key.entityId))
    .filter(({ key }) => !diffOnly || key.kind === "diff")
    .sort((one, two) => SOURCE_ORDER.indexOf(one.key.kind) - SOURCE_ORDER.indexOf(two.key.kind))
    .map(({ key, read }) => ({
      key: keyOf(key.entityId, key.kind, key.sub || ""),
      read: () => Promise.resolve().then(() => read(state.context.active)).catch(() => null),
    }));
}

const rosterJob = ({ entityId, read }) => ({ key: keyOf(entityId, "project-agents"), read });

function foregroundRosterJobs(state, route) {
  // Workspace conversations have their own roster. Only standing on a
  // project's owner makes its complete roster a foreground dependency.
  const owner = routedEntityId(route, state.view);
  return (state.projectRosterReads || [])
    .filter((job) => entityReadsGroup(state, "rosters", job.entityId))
    .filter((job) => job.projectId === route.projectId && (route.name === "project" || job.entityId === owner))
    .map(rosterJob);
}

function foregroundJobs(state) {
  const route = state.route();
  const routed = route.deviceId === state.context.deviceId;
  const jobs = routed ? [...foregroundRosterJobs(state, route), ...routedJobs(state, route)] : [];
  for (const entityId of foregroundDiffEntities(state).filter((id) => entityReadsGroup(state, "checkout", id))) {
    const row = state.pass.rows.get(entityId);
    if (!state.readers.scope(row)) continue;
    jobs.push({ key: keyOf(entityId, "diff"),
      read: () => state.readers.visibleDiff(state.entityContext(entityId), entityId, row, "foreground") });
  }
  if (routed || state.context.recoverWorkingDiff) jobs.push(...sourceJobs(state, !routed));
  if (foregroundTasks(state, route, routed)) {
    jobs.push({ key: keyOf(route.projectId, "tasks-first"), read: () => state.tasks.first(route.projectId) });
  }
  return jobs;
}

function foregroundTasks(state, route, routed) {
  return routed && route.projectId && entityReadsGroup(state, "tasks", route.projectId)
    && projectIds(state.view).includes(route.projectId);
}

function routedJobs(state, route) {
  const entityId = routedEntityId(route, state.view);
  const conversation = conversationFor(route, entityId, state.pass.rows, state.context.deviceId);
  const jobs = conversation && readsThread(state, conversation.entityId, conversation.agent) ? [threadJob(state, conversation.entityId, conversation.agent, "foreground")] : [];
  jobs.push(...scopeJobs(state, entityId, "foreground",
    new Set(["", ...visibleTreePaths(state.context.deviceId, entityId)])));
  return jobs;
}

/** Recovery validates the route even without an open Changes pane, and every
 * shown Changes pane on this device. Ordinary passes retain the routed pane's
 * existing validation; unrelated held diffs remain cold-only background work. */
function foregroundDiffEntities(state) {
  const { deviceId, recoverWorkingDiff } = state.context;
  const route = state.route();
  const routed = route.deviceId === deviceId ? routedEntityId(route, state.view) : null;
  const entities = new Set();
  if (routed && (recoverWorkingDiff || visibleWorkingDiff(deviceId, routed))) entities.add(routed);
  if (recoverWorkingDiff) {
    for (const entityId of state.pass.rows.keys()) {
      if (visibleWorkingDiff(deviceId, entityId)) entities.add(entityId);
    }
  }
  return [...entities];
}

/** Every RPC needed to finish one record has that record's identity. A
 * selected thread's repairs therefore share its page's foreground authority,
 * without maintaining a list of the verbs its reader happens to issue. */
const foregroundRequest = (state, scope) => scope?.deviceId === state.context.deviceId
  && foregroundJobs(state).some((job) => job.key === keyOf(scope.entityId, scope.kind, scope.sub || ""));

async function takeJob(state, job) {
  if (!state.context.active() || state.done.has(job.key)) return;
  // Returning to an in-flight scope joins its read and still waits for the
  // cache write; dispatch alone never makes a foreground requirement complete.
  if (state.pending.has(job.key)) return state.pending.get(job.key);
  const pending = Promise.resolve().then(job.read).finally(() => {
    state.pending.delete(job.key);
    state.done.add(job.key);
  });
  state.pending.set(job.key, pending);
  return pending;
}

const foregroundScope = (state) => JSON.stringify(foregroundJobs(state).map((job) => job.key));

async function drainForeground(state, turn) {
  while (state.context.active() && state.foregroundTurn === turn && foregroundScope(state) === turn.scope) {
    const next = foregroundJobs(state).find((job) => !state.done.has(job.key));
    if (!next) return;
    await takeJob(state, next);
  }
}

function startForeground(state, scope) {
  let supersede;
  const superseded = new Promise((resolve) => { supersede = resolve; });
  const turn = { scope, supersede, promise: null, settled: null };
  const before = state.foregroundTurn;
  state.foregroundTurn = turn;
  before?.supersede();
  // Only the wait is superseded. Existing readers keep their captured cache,
  // push and session authority and can finish writing their own answers.
  const draining = drainForeground(state, turn);
  turn.settled = Promise.race([draining, superseded]);
  turn.promise = turn.settled.then(() => {
    if (state.foregroundTurn !== turn || foregroundScope(state) !== turn.scope) return state.foreground();
  });
  state.context.backgroundStreams?.wake();
  return turn;
}

function currentForeground(state) {
  const scope = foregroundScope(state);
  return state.foregroundTurn?.scope === scope ? state.foregroundTurn : startForeground(state, scope);
}

/** Wait without wire admission, then return a synchronous permission check.
 * Navigation while queued revokes that permission; the scheduler releases its
 * reservation before following a new turn that may need a push continuation. */
async function beforeBackgroundRead(state, scope) {
  while (state.context.active()) {
    const turn = currentForeground(state);
    const ready = () => state.context.active() && state.foregroundTurn === turn
      && foregroundScope(state) === turn.scope;
    if (foregroundRequest(state, scope)) return ready;
    await turn.settled;
    if (ready()) return ready;
  }
}

// Keep the entity boundary: a later roster must not compete with the current
// conversation's repair and push continuations. Discover each roster on entry.
async function readInboxThreads(state, entityId) {
  await drainBackgroundStreams({
    items: (state.pass.rows.get(entityId)?.agents || []).filter((agent) => readsThread(state, entityId, agent)),
    active: state.context.active,
    read: async (agent) => {
      await state.foreground();
      await takeJob(state, threadJob(state, entityId, agent, "background"));
      return true;
    },
  });
}

async function readInbox(state) {
  for (const entityId of state.pass.order) {
    await state.foreground();
    await readInboxThreads(state, entityId);
  }
  for (const projectId of projectIds(state.view).filter((id) => entityReadsGroup(state, "tasks", id))) {
    await state.foreground();
    if (!state.context.active()) return;
    state.done.add(keyOf(projectId, "tasks-first"));
    await state.tasks.rest(projectId);
  }
}

async function readBackgroundEntity(state, entityId) {
  const paths = new Set(["", ...await cachedSubKeys(state.context.deviceId, entityId, "tree")]);
  for (const job of scopeJobs(state, entityId, "background", paths)) {
    await state.foreground();
    await takeJob(state, job);
  }
  await state.foreground();
  if (!state.context.active()) return false;
  return state.readers.extras(state.entityContext(entityId), entityId, state.pass.rows.get(entityId));
}

async function readBackground(state) {
  const reads = (state.sourceTerminalReads || []).filter(({ entityId }) => entityReadsGroup(state, "checkout", entityId)).map(({ entityId, read }) => async () => {
    await state.foreground();
    await takeJob(state, { key: keyOf(entityId, "terminals"), read });
    return true;
  });
  reads.push(...state.pass.order.filter((id) => entityReadsGroup(state, "checkout", id)).map((entityId) => () => readBackgroundEntity(state, entityId)));
  const recovered = await drainBackgroundStreams({
    items: reads,
    active: state.context.active,
    read: (read) => read(),
  });
  await state.foreground();
  return recovered;
}

/** The foreground callback is also called at every background wire boundary
 * and by routeChanged, so a slow background answer never owns the new route. */
export function createSyncPhases(options) {
  const stage = options.stage || (() => {});
  const state = { ...options, done: new Set(), pending: new Map(), foreground: null, foregroundTurn: null };
  state.foreground = () => currentForeground(state).promise;
  return {
    foreground: state.foreground,
    beforeBackgroundRead: (scope) => beforeBackgroundRead(state, scope),
    priorityForRead: (scope, envelope) =>
      envelope?.priority === "background" && foregroundRequest(state, scope)
        ? requestPriorityFields("foreground") : envelope,
    async run() {
      stage("foreground");
      await state.foreground();
      stage("rosters");
      await Promise.all((state.projectRosterReads || []).filter((read) => entityReadsGroup(state, "rosters", read.entityId)).map((read) => takeJob(state, rosterJob(read))));
      stage("foreground");
      await state.foreground();
      stage("inbox");
      await readInbox(state);
      stage("background");
      return readBackground(state);
    },
  };
}
