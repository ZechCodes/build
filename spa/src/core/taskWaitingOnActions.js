// These actions own wire admission and cache writes. Their consumers draw
// cache records, including after a refusal leaves the form's draft intact.
import { contextFor, whenGreeted } from "./deviceContexts.js";
import { bridgeCapabilities } from "./changeEvents.js";
import { cacheTaskWaitingOnBlocks, cacheTaskWaitingOnResponse, captureTaskWaitingOnBlocksRead } from "./taskWaitingOnCache.js";
import { readTaskWaitingOnEverSupported, TASK_WAITING_ON_UPDATE, TASK_WAITING_ON_DOWNGRADED } from "./taskWaitingOnSupport.js";
export { TASK_WAITING_ON_UPDATE };

const isOrdinaryAssignment = (method, params) => method === "tasks.assign" && !Object.hasOwn(params, "retry_deferred_id");
const hasCurrentSupport = (deviceId) => Boolean(contextFor(deviceId)) && bridgeCapabilities(deviceId)?.tasks?.waitingOn === true;

function requireSupport(capable, createsWorkspace, seen, requiresWaitingOn) {
  if (capable) return;
  if (createsWorkspace && seen) throw new Error(TASK_WAITING_ON_DOWNGRADED);
  if (requiresWaitingOn) throw new Error(TASK_WAITING_ON_UPDATE);
}

export function createTaskWaitingOnActions({ deviceId, projectId, taskId, call }) {
  async function send(method, params, options, stands) {
    const assignment = isOrdinaryAssignment(method, params);
    const createsWorkspace = assignment && params.assignee?.kind === "new_workspace";
    const seen = createsWorkspace && await readTaskWaitingOnEverSupported(deviceId);
    // The greeting may change during the cache read. Recheck its authority,
    // then validate and send without another asynchronous gap.
    if (!stands()) throw new Error("This machine's greeting changed before sending. Try again.");
    requireSupport(hasCurrentSupport(deviceId), createsWorkspace, seen, !assignment && options.requiresWaitingOn !== false);
    return call(method, params);
  }

  async function ask(method, params, options = {}) {
    const context = contextFor(deviceId);
    const basis = method === "tasks.waiting_on.blocks"
      ? await captureTaskWaitingOnBlocksRead(deviceId, projectId, taskId) : {};
    // Established commands retain their before-greeting behavior for callers
    // with no session context. New graph contracts always need a greeting.
    const stands = () => !contextFor(deviceId);
    const request = context
      ? await whenGreeted(context, (current) => send(method, params, options, current))
      : { sent: send(method, params, options, stands), stands };
    if (!request) throw new Error("This machine is unavailable. Try again when it returns.");
    const answer = await request.sent;
    if (!request.stands()) throw new Error("This machine's greeting changed while the request was running. Reload the task before trying again.");
    const writeOptions = { ...basis, accept: request.stands, cursor: options.cursor };
    if (method === "tasks.waiting_on.blocks") await cacheTaskWaitingOnBlocks(deviceId, projectId, taskId, answer, writeOptions);
    else await cacheTaskWaitingOnResponse(deviceId, projectId, answer, writeOptions);
    return answer;
  }

  return {
    add: (ids) => ask("tasks.waiting_on.add", { task_id: taskId, waiting_on: [...ids] }),
    remove: (ids) => ask("tasks.waiting_on.remove", { task_id: taskId, waiting_on: [...ids] }),
    blocks: ({ cursor, limit } = {}) => ask("tasks.waiting_on.blocks", {
      task_id: taskId, ...(cursor ? { cursor } : {}), ...(limit ? { limit } : {}),
    }, { cursor }),
    assign: (params) => ask("tasks.assign", {
      ...params, task_id: taskId,
    }),
    retry: (id) => ask("tasks.assign", { task_id: taskId, retry_deferred_id: id }),
    create: (params) => ask("tasks.create", { project_id: projectId, ...params }, {
      requiresWaitingOn: Object.hasOwn(params, "waiting_on"),
    }),
  };
}
