// Negotiated task reads and atomic, page-owned additions to cached history.
import { bridgeCapabilities } from "./changeEvents.js";
import { mergeCachedAtomically } from "./localCache.js";
import { contextFor, whenGreeted } from "./deviceContexts.js";
import { attachTaskTimelineAnchors, taskTimelineFields } from "./taskTimelineWindow.js";
export { mergeTaskTimeline } from "./taskTimelineWindow.js";

export function taskDetailReadParams(deviceId, taskId, before = null) {
  if (bridgeCapabilities(deviceId)?.tasks?.timelinePaged !== true) return before ? null : { task_id: taskId };
  return { task_id: taskId, timeline_paged: true, limit: 50, ...(before ? { before } : {}) };
}

export function taskTimelineAnchorReadParams(deviceId, taskId, anchor) {
  const params = taskDetailReadParams(deviceId, taskId);
  return params.timeline_paged ? { ...params, timeline_anchor: anchor } : null;
}

export const isTaskTimelineCursorFence = (error) => (error?.code || error?.error_code) === "invalid_params"
  && error?.details?.reason === "task_timeline_cursor_invalid";

export function taskDetailRecord(answer) {
  const record = { task: answer?.task || null, timeline: [], ...taskTimelineFields(answer) };
  if (Object.hasOwn(answer || {}, "has_more")) record.timeline_identities = answer.task?.identities || {};
  return record;
}

function sendTaskDetail(options, stands) {
  if (!stands() || !options.active()) return null;
  const { deviceId, taskId, before, anchor } = options;
  const params = anchor ? taskTimelineAnchorReadParams(deviceId, taskId, anchor) : taskDetailReadParams(deviceId, taskId, before);
  const legacy = params === null || params.timeline_paged !== true;
  return { legacy, answer: options.callRpc("tasks.get", params || { task_id: taskId }) };
}

/** Select request shape under the same greeting that owns its cache write. */
export async function requestTaskDetail({ active = () => true, ...options }) {
  const context = contextFor(options.deviceId);
  const direct = () => !contextFor(options.deviceId);
  const request = context ? await whenGreeted(context, (stands) => sendTaskDetail({ ...options, active }, stands))
    : { sent: sendTaskDetail({ ...options, active }, direct), stands: direct };
  const sent = request && await request.sent;
  if (!sent) return null;
  const answer = await sent.answer;
  return { answer, legacy: sent.legacy, accept: () => active() && request.stands() };
}

const sameGeneration = (held, answer) => !held?.timeline_incarnation || held.timeline_incarnation === answer?.timeline_incarnation;
const validPage = (address, answer) => answer?.task?.id === address.sub && Array.isArray(answer.timeline);

export function prependTaskTimeline(address, before, answer, accept = () => true) {
  return mergeCachedAtomically(address, (held) => {
    if (!accept() || !held?.task || held.next_cursor !== before || !validPage(address, answer) || !sameGeneration(held, answer)) return null;
    const rows = new Map(answer.timeline.map((row) => [`${row.type}:${row.id}`, row]));
    for (const row of held.timeline || []) rows.set(`${row.type}:${row.id}`, row);
    const next = { ...held, timeline: [...rows.values()], has_more: answer.has_more === true, next_cursor: answer.next_cursor || null };
    next.timeline_identities = { ...answer.task.identities, ...answer.timeline_identities, ...held.timeline_identities, ...held.task.identities };
    return attachTaskTimelineAnchors(next, held.timeline_anchor_pages);
  });
}

export function keepTaskTimelineAnchor(address, anchor, answer, accept = () => true) {
  return mergeCachedAtomically(address, (held) => {
    if (!accept() || !held?.task || !validPage(address, answer) || !sameGeneration(held, answer)) return null;
    if (!answer.timeline.some((row) => row.id === anchor)) return null;
    const previous = Object.entries(held.timeline_anchor_pages || {}).filter(([id]) => id !== anchor).slice(-3);
    const page = { ...taskTimelineFields(answer), anchor,
      timeline_identities: { ...answer.task.identities, ...held.timeline_identities, ...held.task.identities } };
    return { ...held, timeline_anchor_pages: Object.fromEntries([...previous, [anchor, page]]) };
  });
}
