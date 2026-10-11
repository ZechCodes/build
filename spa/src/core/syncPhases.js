// A pass gives the open conversation and checkout the wire, then fills the
// inbox, and only then warms checkout records nobody is looking at. Route
// intent is resolved between reads, never captured for the entire pass.
import { routedEntityId } from "./inbox.js";
import { visibleTreePaths, visibleWorkingDiff, visibleSyncThread, visibleSyncReads } from "./syncVisibleScopes.js";
import { cachedSubKeys } from "./localCache.js";
import { requestPriorityFields } from "./readRequests.js";
import { drainBackgroundStreams } from "./syncBackgroundStreams.js";

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
  if (!row) return [];
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
    .filter(({ key }) => !state.pass.rows.has(key.entityId))
    .filter(({ key }) => !diffOnly || key.kind === "diff")
    .sort((one, two) => SOURCE_ORDER.indexOf(one.key.kind) - SOURCE_ORDER.indexOf(two.key.kind))
    .map(({ key, read }) => ({
      key: keyOf(key.entityId, key.kind, key.sub || ""),
      read: () => Promise.resolve().then(() => read(state.context.active)).catch(() => null),
    }));
}

function foregroundJobs(state) {
  const route = state.route();
  const routed = route.deviceId === state.context.deviceId;
  const jobs = routed ? routedJobs(state, route) : [];
  for (const entityId of foregroundDiffEntities(state)) {
    const row = state.pass.rows.get(entityId);
    if (!state.readers.scope(row)) continue;
    jobs.push({ key: keyOf(entityId, "diff"),
      read: () => state.readers.visibleDiff(state.entityContext(entityId), entityId, row, "foreground") });
  }
  if (routed || state.context.recoverWorkingDiff) jobs.push(...sourceJobs(state, !routed));
  if (routed && route.projectId && projectIds(state.view).includes(route.projectId)) {
    jobs.push({ key: keyOf(route.projectId, "tasks-first"), read: () => state.tasks.first(route.projectId) });
  }
  return jobs;
}

function routedJobs(state, route) {
  const entityId = routedEntityId(route, state.view);
  const conversation = conversationFor(route, entityId, state.pass.rows, state.context.deviceId);
  const jobs = conversation ? [threadJob(state, conversation.entityId, conversation.agent, "foreground")] : [];
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

/** A queued loader must wake on supersession before following another turn:
 * that new turn may be waiting for this very loader to cross the wire. */
async function beforeBackgroundRead(state, scope) {
  while (state.context.active()) {
    const turn = currentForeground(state);
    if (foregroundRequest(state, scope)) return;
    await turn.settled;
    if (state.foregroundTurn === turn && foregroundScope(state) === turn.scope) return;
  }
}

async function readInbox(state) {
  for (const entityId of state.pass.order) {
    for (const agent of state.pass.rows.get(entityId)?.agents || []) {
      await state.foreground();
      await takeJob(state, threadJob(state, entityId, agent, "background"));
    }
  }
  for (const projectId of projectIds(state.view)) {
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
  const recovered = await drainBackgroundStreams({
    items: state.pass.order,
    active: state.context.active,
    read: (entityId) => readBackgroundEntity(state, entityId),
  });
  await state.foreground();
  return recovered;
}

/** The foreground callback is also called at every background wire boundary
 * and by routeChanged, so a slow background answer never owns the new route. */
export function createSyncPhases(options) {
  const state = { ...options, done: new Set(), pending: new Map(), foreground: null, foregroundTurn: null };
  state.foreground = () => currentForeground(state).promise;
  return {
    foreground: state.foreground,
    beforeBackgroundRead: (scope) => beforeBackgroundRead(state, scope),
    priorityForRead: (scope, envelope) =>
      envelope?.priority === "background" && foregroundRequest(state, scope)
        ? requestPriorityFields("foreground") : envelope,
    async run() {
      await state.foreground();
      await readInbox(state);
      return readBackground(state);
    },
  };
}
