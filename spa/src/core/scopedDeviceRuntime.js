// Device journals own durable facts. Legacy pushes can still enrich existing
// projections with process observations, which the device wire omits deliberately.
import { mergeCachedRecordsTogether } from "./localCache.js";
import { entityIdOf } from "./entityId.js";
import { FEED_COLLECTIONS, stampRow } from "./feedMerge.js";
import { notePush, pushedSince } from "./pushFence.js";
import { deviceRuntimeRowFields as rowFields, deviceRuntimeAgentFields as agentFields,
  deviceRuntimeWorkspaceFields as workspaceFields, deviceRuntimeLifecycleFields as lifecycleRuntimeFields } from "./scopedDeviceFields.js";
const address = (deviceId, kind, entityId = "") => ({ deviceId, entityId, kind });
const changed = (before, after) => JSON.stringify(before) !== JSON.stringify(after) ? after : null;
const pick = (held, incoming, fields) => ({ ...held,
  ...Object.fromEntries(fields.filter((field) => Object.hasOwn(incoming, field)).map((field) => [field, incoming[field]])),
});
const sameGeneration = (left, right) => left.thread_id === right.thread_id
  && String(left.thread_generation_revision ?? "") === String(right.thread_generation_revision ?? "");

function runtimeAgents(held, incoming) {
  const byId = new Map((incoming || []).map((agent) => [agent.id, agent]));
  return held.map((agent) => {
    const next = byId.get(agent.id);
    return next && sameGeneration(agent, next) ? pick(agent, next, agentFields) : agent;
  });
}

function runtimeRow(held, incoming) {
  if (!held) return null;
  const next = pick(held, incoming, rowFields);
  if (Array.isArray(held.agents)) next.agents = runtimeAgents(held.agents, incoming.agents);
  return changed(held, next);
}

function runtimeFeed(held, entityId, incoming) {
  if (!held) return null;
  const next = { ...held };
  for (const field of FEED_COLLECTIONS) {
    if (Array.isArray(held[field])) next[field] = held[field].map((row) =>
      entityIdOf(row) === entityId ? runtimeRow(row, incoming) || row : row);
  }
  return changed(held, next);
}

export function enrichDeviceRuntimeState(deviceId, entityId, incoming, active) {
  return mergeCachedRecordsTogether([address(deviceId, "row", entityId), address(deviceId, "feed"),
    address(deviceId, "project-agents", entityId)], ([row, feed, roster]) => active()
    ? [runtimeRow(row?.value, incoming), runtimeFeed(feed?.value, entityId, incoming), runtimeRow(roster?.value, incoming)] : null);
}

function runtimeWorkspace(held, incoming) {
  const next = pick(held, incoming, workspaceFields);
  if (incoming.lifecycle && typeof incoming.lifecycle === "object") {
    next.lifecycle = pick(held.lifecycle, incoming.lifecycle, lifecycleRuntimeFields);
  }
  return next;
}

function runtimeWorkspaces(held, incoming) {
  const byId = new Map(incoming.map((workspace) => [workspace.id || workspace.workspace_id, workspace]));
  return held.map((workspace) => {
    const next = byId.get(workspace.id || workspace.workspace_id);
    return next ? runtimeWorkspace(workspace, next) : workspace;
  });
}

const runtimeBoardCollections = { pending: "pending", external_worktrees: "externalWorktrees", plans: "plans", runs: "runs" };
export const hasDeviceRuntimeCollections = (incoming) => Object.keys(runtimeBoardCollections).some((field) => Array.isArray(incoming[field]));

// Run bodies still supply review goal/state/base facts. Their conversation
// digests must never compete with device-owned rosters, read marks or sessions.
const runConversationFields = new Set(["agents", "pinned_agent_id", "owned_conversation", "thread", "thread_id",
  "thread_generation_revision", "generation_revision", "read_through_sequence", "watch_started_after", "last_sequence",
  "item_count", "last_event", "agent_id", "conversation_id", "session_started_ms", "last_activity_ms", "retained_last_activity_ms",
  "anchor", "last_activity", "needs_attention", "unread", "unread_count", "unread_reason", "muted", "dismissed", "attention"]);
const runtimeRunBody = (row) => Object.fromEntries(Object.entries(row).filter(([field]) => !runConversationFields.has(field)));

function boardWorkspaceObservations(incoming) {
  const rows = new Map((incoming.workspaces || []).map((row) => [row.id || row.workspace_id, row]));
  for (const summary of incoming.workspace_summaries || []) {
    rows.set(summary.workspace_id, { ...rows.get(summary.workspace_id), workspace_id: summary.workspace_id,
      work_summary: summary.work_summary, can_finish: summary.can_finish === true, finish_blockers: summary.finish_blockers || [] });
  }
  return [...rows.values()];
}

function runtimeBoardFeed(held, incoming, deviceId, workspaces) {
  const next = { ...held };
  let observed = false;
  for (const [wire, field] of Object.entries(runtimeBoardCollections)) {
    if (!Array.isArray(incoming[wire])) continue;
    next[field] = incoming[wire].map((row) => stampRow(field === "runs" ? runtimeRunBody(row) : row, deviceId));
    observed = true;
  }
  if (Array.isArray(held?.workspaces) && workspaces.length) {
    next.workspaces = runtimeWorkspaces(held.workspaces, workspaces);
    observed = true;
  }
  return observed ? changed(held, next) : null;
}

/** Runtime board observations own no device membership. Missing properties
 * are partial pushes; only a present array (including []) replaces its field. */
export function enrichDeviceRuntimeBoard(deviceId, incoming, active, { fence, pushed = false } = {}) {
  const feedAddress = address(deviceId, "feed");
  return mergeCachedRecordsTogether([address(deviceId, "workspaces"), feedAddress], ([workspaces, feed]) => {
    if (!active() || fence !== undefined && pushedSince(feedAddress, fence)) return null;
    const rows = workspaces?.value, view = feed?.value;
    const observations = boardWorkspaceObservations(incoming);
    const nextFeed = runtimeBoardFeed(view, incoming, deviceId, observations);
    if (nextFeed && pushed) notePush(feedAddress);
    return [Array.isArray(rows) && observations.length ? changed(rows, runtimeWorkspaces(rows, observations)) : null,
      nextFeed];
  });
}
