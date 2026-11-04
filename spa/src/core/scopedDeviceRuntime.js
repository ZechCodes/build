// Device journals own durable facts. Legacy pushes can still enrich existing
// projections with process observations, which the device wire omits deliberately.
import { mergeCachedRecordsTogether } from "./localCache.js";
import { entityIdOf } from "./entityId.js";
import { FEED_COLLECTIONS } from "./feedMerge.js";

const rowFields = ["working", "working_time", "agents_running"];
const agentFields = [...rowFields, "state", "active_model", "active_effort", "has_terminal", "can_interrupt",
  "start_error", "surface_session_generation", "surfaces", "last_context_tokens", "last_context_at",
  "session_cache_read_tokens", "compaction", "compact_at_tokens", "max_context_tokens"];
const workspaceFields = ["work_summary", "can_finish", "finish_blockers"];
const lifecycleRuntimeFields = ["measured_at_ms", "last_activity_ms", "idle", "reclaimable", "holds",
  "dirty_files", "unpushed_commits", "behind_commits"];
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

export function enrichDeviceRuntimeBoard(deviceId, incoming, active) {
  if (!Array.isArray(incoming.workspaces)) return Promise.resolve(false);
  return mergeCachedRecordsTogether([address(deviceId, "workspaces"), address(deviceId, "feed")], ([workspaces, feed]) => {
    if (!active()) return null;
    const rows = workspaces?.value, view = feed?.value;
    return [Array.isArray(rows) ? changed(rows, runtimeWorkspaces(rows, incoming.workspaces)) : null,
      Array.isArray(view?.workspaces) ? changed(view, { ...view, workspaces: runtimeWorkspaces(view.workspaces, incoming.workspaces) }) : null];
  });
}
