// Task pages share one walk across the open project's first page and the
// inbox phase. Both phases use the sync layer's task-read serialization;
// pages still land, read back and fold through trackerPages' authority rules.
import { createTaskPagePull, foldTasksPage, pagesTasks } from "./trackerPages.js";
import { nextTaskRead } from "./taskReadOrder.js";
import { readTasksRecord, tasksAddress, tasksRecord, writeTasksRecord } from "./trackerCache.js";
import { writeListedUserSession } from "./userSessionCache.js";

async function projectColumns(context, projectId, ask, priority) {
  const held = await readTasksRecord(context.deviceId, projectId);
  if (held?.columns?.length) return held.columns;
  if (!context.active()) return [];
  return (await ask(context, "tasks.columns", { project_id: projectId }, priority))?.columns || [];
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
  const read = await nextTaskRead();
  if (!context.active()) return false;
  const answer = await ask(context, "tasks.list", { project_id: projectId }, priority);
  if (!answer || !context.active()) return false;
  const value = await columns;
  if (!context.active()) return false;
  await Promise.all([
    writeTasksRecord(context.deviceId, projectId, tasksRecord(answer.tasks, value, read)),
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

function projectPhases(context, projectId, { ask, readReviews }) {
  let priority = "foreground";
  let pull = null;
  let firstPromise = null;
  let restPromise = null;
  const begin = () => {
    const columns = projectColumns(context, projectId, ask, priority);
    if (!pagesTasks(context.deviceId)) return firstWithColumns(wholeList(context, projectId, ask, columns, priority), columns);
    pull = createTaskPagePull({
      deviceId: context.deviceId, projectId, params: { project_id: projectId },
      active: () => context.active(),
      ask: (params) => ask(context, "tasks.list", params, priority),
      fold: (stretch, page) => foldPage(context, projectId, columns, stretch, page),
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
