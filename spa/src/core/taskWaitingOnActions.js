// These actions own wire admission and cache writes. Their consumers draw
// cache records, including after a refusal leaves the form's draft intact.
import { contextFor, whenGreeted } from "./deviceContexts.js";
import { bridgeCapabilities } from "./changeEvents.js";
import { cacheTaskWaitingOnBlocks, cacheTaskWaitingOnResponse } from "./taskWaitingOnCache.js";

export const TASK_WAITING_ON_UPDATE = "Update this device's bridge to edit prerequisites or save assignments that wait.";

export function createTaskWaitingOnActions({ deviceId, projectId, taskId, call }) {
  async function ask(method, params, { requiresWaitingOn = true, cursor } = {}) {
    const context = contextFor(deviceId);
    // Established commands retain their before-greeting behavior for callers
    // with no session context. New graph contracts always need a greeting.
    if (!context && !requiresWaitingOn) {
      const answer = await call(method, params);
      await cacheTaskWaitingOnResponse(deviceId, projectId, answer);
      return answer;
    }
    const request = await whenGreeted(context, () => {
      if (requiresWaitingOn && bridgeCapabilities(deviceId)?.tasks?.waitingOn !== true) {
        throw new Error(TASK_WAITING_ON_UPDATE);
      }
      return call(method, params);
    });
    if (!request) throw new Error("This machine is unavailable. Try again when it returns.");
    const answer = await request.sent;
    if (!request.stands()) throw new Error("This machine's greeting changed while the request was running. Reload the task before trying again.");
    const options = { accept: request.stands, cursor };
    if (method === "tasks.waiting_on.blocks") await cacheTaskWaitingOnBlocks(deviceId, projectId, taskId, answer, options);
    else await cacheTaskWaitingOnResponse(deviceId, projectId, answer, options);
    return answer;
  }

  return {
    add: (ids) => ask("tasks.waiting_on.add", { task_id: taskId, waiting_on: [...ids] }),
    remove: (ids) => ask("tasks.waiting_on.remove", { task_id: taskId, waiting_on: [...ids] }),
    blocks: ({ cursor, limit } = {}) => ask("tasks.waiting_on.blocks", {
      task_id: taskId, ...(cursor ? { cursor } : {}), ...(limit ? { limit } : {}),
    }, { cursor }),
    assign: (params, { requiresWaitingOn = false } = {}) => ask("tasks.assign", {
      ...params, task_id: taskId,
    }, { requiresWaitingOn }),
    retry: (id) => ask("tasks.assign", { task_id: taskId, retry_deferred_id: id }),
    create: (params) => ask("tasks.create", { project_id: projectId, ...params }, {
      requiresWaitingOn: Object.hasOwn(params, "waiting_on"),
    }),
  };
}
