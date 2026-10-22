// Wire effects name domain facts. Only this allowlist translates them to local
// cache layers; addresses, paths and patch instructions never come from wire.
import { cachedAddresses } from "./localCache.js";
import { taskAddress, TRACKER_TASK_KIND } from "./trackerCache.js";
import { threadCacheAddress } from "./conversationCache.js";
import { provisionalThreadItem } from "./thread.js";

const paramsOf = (record) => record.request?.params || record.params || {};
const methodOf = (record) => record.request?.method || record.method;
const resourceFields = ["task_id", "comment_id", "workspace_id", "agent_id", "conversation_id", "thread_id", "message_id"];
const commentFields = ["author", "attachments", "refs", "mentions_user", "reply_to", "author_context"];
const fieldsOf = (source, fields) => Object.fromEntries(fields.filter((key) => source?.[key] !== undefined).map((key) => [key, source[key]]));
const wireEffects = (frame) => Array.isArray(frame.result?.overlay) ? frame.result.overlay : [];
const localTimestamp = (record) => new Date(record.createdAt ?? Date.now()).toISOString();

function reservations(frame, record, typed) {
  return Object.assign({}, record.reservedIds, fieldsOf(frame.result, resourceFields),
    ...typed.map((effect) => fieldsOf(effect, resourceFields)));
}

function projectFromFrame(frame) {
  const owners = frame.write?.owners || frame.published || [];
  return owners.find((owner) => owner.kind === "project")?.id || frame.result?.task?.project_id;
}

/** Task ownership comes from domain metadata or a known task address, never
 * from a bridge-provided cache address. Ambiguous cached ownership is ignored. */
async function taskProject(frame, record, taskId) {
  const project = projectFromFrame(frame) || paramsOf(record).project_id;
  if (project) return project;
  const held = await cachedAddresses({ deviceId: record.deviceId });
  const matches = held.filter((address) => address.kind === TRACKER_TASK_KIND && address.sub === taskId);
  return matches.length === 1 ? matches[0].entityId : null;
}

function commentValue(effect, frame, record, taskId, commentId) {
  return {
    type: "comment", id: commentId, task_id: taskId,
    author: { kind: "user" }, attachments: [], refs: [],
    ...fieldsOf(effect, commentFields), body: effect.body,
    created_at: effect.created_at || frame.result?.accepted_at || localTimestamp(record),
  };
}

async function taskComment(effect, frame, record) {
  const taskId = effect.task_id || frame.result?.task_id || paramsOf(record).task_id;
  const commentId = effect.comment_id || frame.result?.comment_id;
  if (!taskId || !commentId || typeof effect.body !== "string") return [];
  const projectId = await taskProject(frame, record, taskId);
  if (!projectId) return [];
  const value = commentValue(effect, frame, record, taskId, commentId);
  return [{ address: taskAddress(record.deviceId, projectId, taskId), kind: "upsert", path: ["timeline"], key: "id", value }];
}

function threadIdentity(record) {
  const params = paramsOf(record);
  return {
    deviceId: record.deviceId, entityId: params.entity_id,
    agentId: params.agent_id, conversationId: params.conversation_id, threadId: params.thread_id,
  };
}

const messageSequence = (normalized, frame) => normalized.sequence ?? normalized.posted_sequence ?? frame.result?.posted_sequence ?? null;
const messageIdentity = (effect, frame) => effect?.message?.message_id || effect?.message_id || frame.result?.message_id;

function threadItem(effect, frame, record, deliveryStatus) {
  const params = paramsOf(record);
  const normalized = effect?.message || effect || {};
  const message = { ...record.draft, ...params, ...fieldsOf(normalized, ["body", "attachments", "viewing_context"]) };
  const sequence = messageSequence(normalized, frame);
  const value = provisionalThreadItem({ operationId: record.operationId, message, sequence, deliveryStatus });
  value.data.created_at = normalized.created_at || frame.result?.accepted_at || localTimestamp(record);
  return value;
}

function threadMessage(effect, frame, record, deliveryStatus = "sent") {
  const identity = threadIdentity(record);
  if (!identity.entityId || !(identity.conversationId || identity.agentId)) return [];
  const value = threadItem(effect, frame, record, deliveryStatus);
  const messageId = messageIdentity(effect, frame);
  if (messageId) value.data.id = messageId;
  return [{ address: threadCacheAddress(identity), kind: "upsert", path: ["items"], key: ["data", "operation_id"], insertOnly: true, value }];
}

const normalizers = {
  "tasks.comment:task_comment": taskComment,
  "thread.post:thread_message": threadMessage,
};

function confirmationEffects(frame, record) {
  if (frame.type !== "writes.confirmed" || frame.state !== "succeeded" || methodOf(record) !== "tasks.comment") return [];
  const comment = frame.result?.comment;
  if (!comment) return [];
  return [{
    kind: "task_comment", comment_id: comment.id,
    task_id: comment.task_id || frame.result?.task?.id,
    ...fieldsOf(comment, [...commentFields, "body", "created_at"]),
  }];
}

function recognizedEffects(frame, record) {
  const method = methodOf(record);
  const effects = [...wireEffects(frame), ...confirmationEffects(frame, record)];
  return effects.filter((effect) => effect && Object.hasOwn(normalizers, `${method}:${effect.kind}`));
}

/** Persist an optional local message layer before transport. Complete task or
 * conversation records are never replaced by an editor's draft. */
export async function normalizeIntentEffects(record) {
  const overlay = methodOf(record) === "thread.post" ? threadMessage(null, {}, record, "queued") : [];
  return { overlay, reservedIds: record.reservedIds || {} };
}

/** Normalize accepted domain facts. Unknown effect kinds stay in the raw ack
 * stored by the write service, without becoming executable cache patches. */
export async function normalizeAckEffects(frame, record) {
  const typed = recognizedEffects(frame, record);
  const normalized = await Promise.all(typed.map((effect) => normalizers[`${methodOf(record)}:${effect.kind}`](effect, frame, record)));
  const overlay = normalized.flat();
  if (!typed.length && methodOf(record) === "thread.post") overlay.push(...threadMessage(null, frame, record));
  return { overlay, reservedIds: reservations(frame, record, typed) };
}
