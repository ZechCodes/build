// One ownership list for runtime enrichment and device capture admission.
// Everything outside these fields remains guarded by the original cache write.
export const deviceRuntimeRowFields = ["working", "working_time", "agents_running"];
export const deviceRuntimeAgentFields = [...deviceRuntimeRowFields, "state", "active_model", "active_effort", "has_terminal", "can_interrupt",
  "start_error", "surface_session_generation", "surfaces", "last_context_tokens", "last_context_at",
  "session_cache_read_tokens", "compaction", "compact_at_tokens", "max_context_tokens"];
export const deviceRuntimeWorkspaceFields = ["work_summary", "can_finish", "finish_blockers"];
export const deviceRuntimeLifecycleFields = ["measured_at_ms", "last_activity_ms", "idle", "reclaimable", "holds",
  "dirty_files", "unpushed_commits", "behind_commits"];

const without = (value, fields) => value && typeof value === "object"
  ? Object.fromEntries(Object.entries(value).filter(([key]) => !fields.includes(key))) : value;

function ownedRow(value) {
  const next = without(value, deviceRuntimeRowFields);
  if (Array.isArray(next?.agents)) next.agents = next.agents.map((agent) => without(agent, deviceRuntimeAgentFields));
  return next;
}

function ownedWorkspace(value) {
  const next = without(value, [...deviceRuntimeWorkspaceFields, ...deviceRuntimeRowFields]);
  if (next && Object.hasOwn(next, "lifecycle")) {
    const lifecycle = without(next.lifecycle, deviceRuntimeLifecycleFields);
    if (lifecycle && Object.values(lifecycle).some((value) => value !== undefined)) next.lifecycle = lifecycle;
    else delete next.lifecycle;
  }
  return next;
}

const ownedWorkspaces = (value) => Array.isArray(value) ? value.map(ownedWorkspace) : value;
function ownedFeed(value) {
  if (!value) return value;
  const next = { ...value };
  for (const field of ["items", "runs", "plans", "externalWorktrees", "external_worktrees", "pending", "projects"]) {
    if (Array.isArray(next[field])) next[field] = next[field].map(ownedRow);
  }
  if (Array.isArray(next.workspaces)) next.workspaces = ownedWorkspaces(next.workspaces);
  return next;
}

function comparable(value) {
  if (Array.isArray(value)) return value.map(comparable);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(Object.keys(value).filter((key) => key !== "__cacheObserved").sort()
    .map((key) => [key, comparable(value[key])]));
}

const projections = { row: ownedRow, "project-agents": ownedRow, workspaces: ownedWorkspaces, feed: ownedFeed,
  "user-session": (value) => without(value, ["now_ms", "gap_ms"]) };

/** A changed write is harmless only when this exact owned projection is equal. */
export function deviceOwnedProjection(kind, value) {
  const project = projections[kind];
  return project ? JSON.stringify(comparable(project(value))) : undefined;
}
