// Shapes emitted by device_lifecycle.rs, device_producers.rs, entities.rs and
// device_families.rs. IDs, names and paths are synthetic.
export const deviceRecord = (kind, key, row, revision = "1", extra = {}) => ({ kind, key, revision, row, ...extra });
export function deviceRecords() {
  const record = deviceRecord;
  return [
    record("project", "proj-1", { id: "proj-1", project_id: "proj-1", name: "Synthetic project", path: "/synthetic/project", base_branch: "dev", next_source: 2 }),
    record("project_source", "proj-1/source-1", { id: "source-1", project_id: "proj-1", name: "Source", path: "/synthetic/project", mount: ".", is_git: true, base_branch: "dev" }),
    record("workspace", "workspace-1", { id: "workspace-1", project_id: "proj-1", name: "Synthetic workspace", root: "/synthetic/workspace", status: "ready", isolation: "worktree", managed: true, created_by_agent: false, locked: false }),
    record("workspace_directory", "workspace-1/dir-1", { id: "dir-1", workspace_id: "workspace-1", project_id: "proj-1", source_id: "source-1", name: "Source", path: "/synthetic/workspace/source", is_git: true, branch: "synthetic", status: "ready", source_path: "/synthetic/project", base_branch: "dev" }),
    record("badge", "device", { projects: 1, workspaces: 1, locked_workspaces: 0 }),
    record("agent", "agent-1", { id: "agent-1", owner_id: "owner-1", project_id: "proj-1", workspace_id: "workspace-1", conversation_id: "agent-1", thread_id: "thread-1", thread_generation_revision: "1", ordinal: 1, provider: "codex", model: "synthetic-model", effort: "medium", watched: true, choice_revision: 1, created_at: "2026-11-01T12:00:00Z", title: "Synthetic agent", topic: "Synthetic agent", name: "Synthetic agent", created_by: null, read_through_sequence: "0", watch_started_after: "0", last_sequence: "5", unread_count: 2, unread_reason: "reply" }),
    record("identity", "agent-1", { agent_id: "agent-1", name: "Synthetic agent", project_role: null, ordinal: 1, provider: "codex", workspace_id: null, workspace_name: "Synthetic workspace" }),
    record("inbox_conversation", "agent-1", { owner_id: "owner-1", agent_id: "agent-1", thread_id: "thread-1", thread_generation_revision: "1", project_id: "proj-1", workspace_id: "workspace-1", title: "Synthetic agent", watched: true, muted: false, dismissed: false, unread_count: 2, read_through_sequence: "0", last_sequence: "5", watch_started_after: "0", unread_reason: "reply", last_activity_ms: 1793534400000, session_started_ms: 1793534400000, last_message_summary: "Synthetic reply", pending: false, last_interaction_at: null, resume_at: null }),
    record("inbox_task", "task-1", { id: "task-1", project_id: "proj-1", number: 1, title: "Synthetic task", state: "open", status: "backlog", labels: [], priority: "normal", assignee: null, created_by: { kind: "user" }, trackers: [], links: { workspace_ids: [], parent_task_id: null }, dependencies: { blocked: false, waiting_on_count: 0, blocks_count: 0 }, attachment_count: 0, summary_truncated: false, closed_at: null, done_at: null, watched: true, unread_count: 0, unread_mentions: 0, unread_agent_comments: 0, created_at: "2026-11-01T12:00:00Z", updated_at: "2026-11-01T12:00:00Z", selection: { pending_present: false } }),
    record("inbox_capture", "capture-1", { id: "capture-1", project_id: null, title: "Synthetic capture", state: "unrouted", created_at: "2026-11-01T12:00:00Z", updated_at: "2026-11-01T12:00:00Z", needs_answer: true, error: null }),
    record("user_session", "current", { session_started_ms: 1793534400000, last_activity_ms: 1793534400000, previous_session_ended_ms: null }),
    record("archived_worktree", "archive-1", { id: "archive-1", project_id: "proj-1", name: "Synthetic archive", path: "/synthetic/archive", branch: "finished", head_sha: "synthetic-head", action: "delete", status: "archived", archived_at: "2026-11-01T12:00:00Z" }),
    record("workspace_lifecycle", "workspace-1", { workspace_id: "workspace-1", pruned_bytes: 100, pruned_at_ms: 1793534400000, noticed_at_ms: null, size_bytes: 200, size_measured_at_ms: 1793534400000 }),
    record("owner_session", "owner-1", { owner_id: "owner-1", project_id: "proj-1", workspace_id: "workspace-1", pinned_agent_id: "agent-1", session_started_ms: 1793534400000, last_activity_ms: 1793534400000, retained_last_activity_ms: 1793534300000 }),
    record("project_session", "proj-1", { project_id: "proj-1", session_started_ms: 1793534400000, last_activity_ms: 1793534400000, retained_last_activity_ms: 1793534300000 }),
  ];
}

export function projectOwnerRecords({ pin = "coordinator", roles = true } = {}) {
  const agent = deviceRecords().find((record) => record.kind === "agent").row;
  const worker = { ...agent, id: "worker", conversation_id: "worker", thread_id: "thread-worker", owner_id: "project-owner", workspace_id: null,
    ordinal: 1, topic: "Worker", title: "Worker", name: "Worker" };
  const coordinator = { ...worker, id: "coordinator", conversation_id: "coordinator", thread_id: "thread-coordinator", ordinal: 3,
    topic: "Coordinator", title: "Coordinator", name: "Coordinator" };
  return [deviceRecord("agent", worker.id, worker), deviceRecord("agent", coordinator.id, coordinator),
    ...[worker, coordinator].map((agent) => deviceRecord("identity", agent.id, { agent_id: agent.id, name: agent.topic,
      ordinal: agent.ordinal, workspace_id: null, workspace_name: null, provider: agent.provider,
      project_role: roles ? agent.id === pin ? "coordinator" : "worker" : null })),
    deviceRecord("owner_session", "project-owner", { owner_id: "project-owner", project_id: "proj-1", workspace_id: null,
      pinned_agent_id: pin, session_started_ms: null, last_activity_ms: null, retained_last_activity_ms: null })];
}
