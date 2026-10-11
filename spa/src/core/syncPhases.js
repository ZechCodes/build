// A pass gives the open conversation and checkout the wire, then fills the
// inbox, and only then warms checkout records nobody is looking at. Route
// intent is resolved between reads, never captured for the entire pass.
import { routedEntityId } from "./inbox.js";
import { visibleTreePaths, visibleWorkingDiff, visibleSyncThread, visibleSyncReads } from "./syncVisibleScopes.js";
import { cachedSubKeys } from "./localCache.js";
import { requestPriorityFields } from "./readRequests.js";

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

function scopeJobs(state, entityId, priority, paths, workingDiff) {
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
    if (workingDiff) jobs.push(job("diff", "", state.readers.visibleDiff));
  }
  return jobs;
}

const threadJob = (state, entityId, agent, priority) => ({
  key: keyOf(entityId, "thread", agent.conversation_id || agent.id),
  read: () => state.readers.thread(state.entityContext(entityId), entityId, agent, priority),
});

const SOURCE_ORDER = ["status", "tree", "terminals", "log", "unpushed", "diff"];
function sourceJobs(state) {
  return visibleSyncReads(state.context.deviceId)
    .filter(({ key }) => !state.pass.rows.has(key.entityId))
    .sort((one, two) => SOURCE_ORDER.indexOf(one.key.kind) - SOURCE_ORDER.indexOf(two.key.kind))
    .map(({ key, read }) => ({
      key: keyOf(key.entityId, key.kind, key.sub || ""),
      read: () => Promise.resolve().then(() => read(state.context.active)).catch(() => null),
    }));
}

function foregroundJobs(state) {
  const route = state.route();
  if (route.deviceId !== state.context.deviceId) return [];
  const entityId = routedEntityId(route, state.view);
  const conversation = conversationFor(route, entityId, state.pass.rows, state.context.deviceId);
  const jobs = conversation ? [threadJob(state, conversation.entityId, conversation.agent, "foreground")] : [];
  jobs.push(...scopeJobs(state, entityId, "foreground",
    new Set(["", ...visibleTreePaths(state.context.deviceId, entityId)]),
    visibleWorkingDiff(state.context.deviceId, entityId)));
  jobs.push(...sourceJobs(state));
  if (route.projectId && projectIds(state.view).includes(route.projectId)) {
    jobs.push({ key: keyOf(route.projectId, "tasks-first"), read: () => state.tasks.first(route.projectId) });
  }
  return jobs;
}

const matchesScope = (scope, params) => Boolean(scope)
  && Object.entries(scope).every(([key, value]) => params[key] === value);

function selectedThreadRequest(state, route, entityId, params) {
  const selected = conversationFor(route, entityId, state.pass.rows, state.context.deviceId);
  return selected?.entityId === params.entity_id && selected?.agent.id === params.agent_id;
}

const FOREGROUND_CHECKOUT_READS = {
  "git.status": (_state, _entityId, matches) => matches,
  "git.log": (_state, _entityId, matches) => matches,
  "git.unpushed": (_state, _entityId, matches) => matches,
  "fs.tree": (state, entityId, matches, params) => matches
    && new Set(["", ...visibleTreePaths(state.context.deviceId, entityId)]).has(params.path || ""),
  "run.diff": (state, entityId, matches) => matches && visibleWorkingDiff(state.context.deviceId, entityId),
  "worktree.diff": (state, entityId, matches) => matches && visibleWorkingDiff(state.context.deviceId, entityId),
};

function foregroundRequest(state, method, params) {
  const route = state.route();
  if (route.deviceId !== state.context.deviceId) return false;
  if (method === "tasks.list" || method === "tasks.columns") return params.project_id === route.projectId && !params.cursor;
  const entityId = routedEntityId(route, state.view);
  if (method === "thread.page") return selectedThreadRequest(state, route, entityId, params);
  const row = state.pass.rows.get(entityId);
  if (method === "term.list") return matchesScope(state.readers.terminalScope(row), params);
  const matches = matchesScope(state.readers.scope(row), params);
  return FOREGROUND_CHECKOUT_READS[method]?.(state, entityId, matches, params) || false;
}

async function takeJob(state, job) {
  if (!state.context.active() || state.done.has(job.key)) return;
  // An already dispatched background read needs no second request when its
  // checkout becomes foreground. All of its writes retain their own fences.
  state.done.add(job.key);
  return job.read();
}

async function drainForeground(state) {
  while (state.context.active()) {
    const next = foregroundJobs(state).find((job) => !state.done.has(job.key));
    if (!next) return;
    await takeJob(state, next);
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

async function readBackground(state) {
  let recovered = true;
  for (const entityId of state.pass.order) {
    const paths = new Set(["", ...await cachedSubKeys(state.context.deviceId, entityId, "tree")]);
    for (const job of scopeJobs(state, entityId, "background", paths, false)) {
      await state.foreground();
      await takeJob(state, job);
    }
    await state.foreground();
    if (!state.context.active()) return false;
    const files = await state.readers.extras(state.entityContext(entityId), entityId, state.pass.rows.get(entityId));
    recovered = files && recovered;
  }
  await state.foreground();
  return recovered;
}

/** The foreground callback is also called at every background wire boundary
 * and by routeChanged, so a slow background answer never owns the new route. */
export function createSyncPhases(options) {
  const state = { ...options, done: new Set(), foreground: null };
  let foreground = null;
  state.foreground = () => {
    if (!foreground) foreground = drainForeground(state).finally(() => { foreground = null; });
    return foreground;
  };
  return {
    foreground: state.foreground,
    priorityForRead: (method, params, envelope) =>
      envelope?.priority === "background" && foregroundRequest(state, method, params)
        ? requestPriorityFields("foreground") : envelope,
    async run() {
      await state.foreground();
      await readInbox(state);
      return readBackground(state);
    },
  };
}
