// These actions own wire admission and cache writes. Their consumers draw
// cache records, including after a refusal leaves the form's draft intact.
import { contextFor, whenGreeted } from "./deviceContexts.js";
import { bridgeCapabilities } from "./changeEvents.js";
import { cacheTaskWaitingOnBlocks, cacheTaskWaitingOnResponse, captureTaskWaitingOnBlocksRead } from "./taskWaitingOnCache.js";
import { TASK_WAITING_ON_UPDATE } from "./taskWaitingOnSupport.js";
import { readTaskAssignmentFacts } from "./taskAssignmentFacts.js";
export { TASK_WAITING_ON_UPDATE };

export function createTaskWaitingOnActions({ deviceId, projectId, taskId, task = null, call }) {
  async function needsWaitingOn(method, params, options) {
    if (method !== "tasks.assign" || Object.hasOwn(params, "retry_deferred_id")) return options.requiresWaitingOn !== false;
    if (params.assignee === null) return false;
    const facts = await readTaskAssignmentFacts(deviceId, projectId, taskId, task);
    return Boolean(facts?.dependencies?.blocked);
  }

  async function send(method, params, options, stands) {
    // This runs after greeting settlement, not at submit: cache facts may
    // change while greeting. Retain its authority across the cache read.
    const requiresWaitingOn = await needsWaitingOn(method, params, options);
    if (!stands()) throw new Error("This machine's greeting changed before sending. Try again.");
    if (requiresWaitingOn && (!contextFor(deviceId) || bridgeCapabilities(deviceId)?.tasks?.waitingOn !== true)) {
      throw new Error(TASK_WAITING_ON_UPDATE);
    }
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
