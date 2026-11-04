// Task pages share one walk across the open project's first page and the
// inbox phase. Both phases use the sync layer's task-read serialization;
// pages still land, read back and fold through trackerPages' authority rules.
import { projectScopedSyncOwns, scopedSyncWriter, wakeScopedSync } from "./scopedSyncRuntime.js";
import { createTaskPagePull, foldTasksPage, pagesTasks } from "./trackerPages.js";
import { captureTaskContentRead, observeTaskContent } from "./taskContentAuthority.js";
import { readTasksRecord, tasksAddress, tasksRecord, writeTasksRecord } from "./trackerCache.js";
import { refreshScopedProjectColumns } from "./scopedProjectColumns.js";
import { writeListedUserSession } from "./userSessionCache.js";

/** Continuations stay background even when the project's first page is shown. */
export const taskReadScope = (context, projectId, params = {}) => ({
  deviceId: context.deviceId, entityId: projectId,
  kind: params.cursor ? "tasks-rest" : "tasks-first",
});

async function projectColumns(context, projectId, ask, priority) {
  const held = await readTasksRecord(context.deviceId, projectId);
  if (held?.columns?.length) return held.columns;
  if (!context.active()) return [];
  return (await ask(context, "tasks.columns", { project_id: projectId }, priority, taskReadScope(context, projectId)))?.columns || [];
}

async function foldPage(context, projectId, columns, stretch, page) {
  const value = await columns;
  if (!context.active()) return false;
  const [folded] = await Promise.all([
    foldTasksPage(tasksAddress(context.deviceId, projectId), stretch, () => value),
    writeListedUserSession(context.deviceId, page),
  ]);
  return folded;
}

async function wholeList(context, projectId, ask, columns, priority) {
  const contentRead = await captureTaskContentRead(tasksAddress(context.deviceId, projectId), { revalidate: "full" });
  if (!context.active()) return false;
  const answer = await ask(context, "tasks.list", { project_id: projectId }, priority, taskReadScope(context, projectId));
  if (!answer || !context.active()) return false;
  const value = await columns;
  if (!context.active()) return false;
  const observed = (answer.tasks || []).map((task) => observeTaskContent(task, { contentRead, coverage: "full" }));
  await Promise.all([
    writeTasksRecord(context.deviceId, projectId, tasksRecord(observed, value, contentRead.readOrder), { accept: () => context.active(), contentRead }),
    writeListedUserSession(context.deviceId, answer),
  ]);
  return true;
}

// Columns and the first page start together. Even a refused or failed page
// must drain its sibling before the inbox can hand the wire to checkout work.
async function firstWithColumns(first, columns) {
  const reads = await Promise.allSettled([first, columns]);
  const failure = reads.find((read) => read.status === "rejected");
  if (failure) throw failure.reason;
  return reads[0].value;
}

const scopedProjectColumns = (context, projectId, ask, priority) => refreshScopedProjectColumns({
  deviceId: context.deviceId, projectId, active: () => context.active() && scopedSyncWriter(),
  callRpc: (method, params) => ask(context, method, params, priority, taskReadScope(context, projectId)),
});

function projectPhases(context, projectId, { ask, readReviews }) {
  let priority = "foreground";
  let pull = null;
  let firstPromise = null;
  let restPromise = null;
  const begin = () => {
    if (projectScopedSyncOwns(context.deviceId, projectId)) {
      return Promise.all([wakeScopedSync(context.deviceId, (scope) => scope.project_id === projectId),
        scopedProjectColumns(context, projectId, ask, priority)]).then(() => true);
    }
    const owner = context;
    const reading = { ...owner, active: () => owner.active() && !projectScopedSyncOwns(owner.deviceId, projectId) };
    const columns = projectColumns(reading, projectId, ask, priority);
    if (!pagesTasks(context.deviceId)) return firstWithColumns(wholeList(reading, projectId, ask, columns, priority), columns);
    pull = createTaskPagePull({
      deviceId: context.deviceId, projectId, params: { project_id: projectId },
      active: reading.active,
      ask: (params) => ask(context, "tasks.list", params, priority, taskReadScope(context, projectId, params)),
      fold: (stretch, page) => foldPage(reading, projectId, columns, stretch, page),
    });
    return firstWithColumns(pull.first(), columns);
  };
  const first = (nextPriority) => {
    priority = nextPriority;
    if (!context.active()) return Promise.resolve(false);
    return (firstPromise ||= begin());
  };
  const finish = async (nextPriority) => {
    if (!await first(nextPriority) || !context.active()) return false;
    if (projectScopedSyncOwns(context.deviceId, projectId)) return true;
    if (pull && !await pull.rest()) return false;
    if (!context.active()) return false;
    await readReviews(context, projectId, nextPriority);
    return true;
  };
  return { first, rest: (nextPriority) => (restPromise ||= finish(nextPriority)) };
}

/** One task continuation per project for this pass. `serialize` shares the
 * session's task reader with pushes without holding it across phase gaps. */
export function createTaskPhases(context, dependencies) {
  const projects = new Map();
  const serialize = dependencies.serialize || ((_projectId, operation) => operation());
  const run = (projectId, phase, priority) => {
    if (!projectId || !context.active()) return Promise.resolve(false);
    if (!projects.has(projectId)) projects.set(projectId, projectPhases(context, projectId, dependencies));
    return serialize(projectId, () => projects.get(projectId)[phase](priority));
  };
  return {
    first: (projectId, priority = "foreground") => run(projectId, "first", priority),
    rest: (projectId, priority = "background") => run(projectId, "rest", priority),
  };
}
