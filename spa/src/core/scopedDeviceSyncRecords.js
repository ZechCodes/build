// Device rows are durable summaries, never live process or filesystem state.
// Rebuild the existing cache projections; mounted views never read this wire.
import { stampProject, stampRow, stampWorkspace } from "./feedMerge.js";

export const deviceRecordId = (kind, key) => JSON.stringify([kind, key]);
// Change pages overlay a historical operation on the key's CURRENT row and
// membership. An old delete can therefore carry today's recreated live row.
export const removedDeviceRecord = (record) => record.row === null && record.member !== true;
export const deviceRecordAddress = (deviceId, kind, key) => ({ deviceId, entityId: "", kind: "sync-device-record", sub: deviceRecordId(kind, key) });
export const deviceRoot = (deviceId, kind, entityId = "", sub = "") => ({ deviceId, entityId, kind, sub });
const valuesOf = (rows, kind) => [...rows.values()].filter((entry) => entry.kind === kind && !entry.deleted).map((entry) => entry.row);
const sameThread = (a, b) => a?.thread_id === b?.thread_id
  && String(a?.thread_generation_revision ?? "") === String(b?.thread_generation_revision ?? "");
const decimal = (value) => /^\d+$/.test(String(value)) ? BigInt(value) : null;

// #881: a local read report remains authoritative until this exact thread's
// bridge cursor catches up. Decimal sequence strings must never round to Number.
export function preserveConversationRead(held, incoming) {
  if (!sameThread(held, incoming)) return incoming;
  const before = decimal(held.read_through_sequence), next = decimal(incoming.read_through_sequence);
  if (before === null || next === null || before <= next) return incoming;
  return { ...incoming, read_through_sequence: held.read_through_sequence,
    unread_count: held.unread_count, unread_reason: held.unread_reason };
}

function agentProjection(agent, conversation, identity, previous) {
  const incoming = { ...previous, ...agent, ...conversation, id: agent.id,
    conversation_id: agent.conversation_id, project_role: identity?.project_role ?? null };
  return preserveConversationRead(previous, incoming);
}

const ownerIdOf = (agents, field, id) => agents.find((agent) => agent[field] === id)?.owner_id;
const mergeList = (held, incoming, deleted, idOf) => {
  const next = new Map((held || []).filter((row) => !deleted.has(idOf(row))).map((row) => [idOf(row), row]));
  for (const row of incoming) next.set(idOf(row), { ...next.get(idOf(row)), ...row });
  return [...next.values()];
};
const deletedKeys = (rows, kind) => new Set([...rows.values()].filter((entry) => entry.kind === kind && entry.deleted).map((entry) => entry.key));

function projectOwners(rows, held) {
  const retired = deletedKeys(rows, "owner_session");
  const selected = heldProjectOwners(held("projects"), retired);
  const identities = new Map(valuesOf(rows, "identity").map((row) => [row.agent_id, row]));
  for (const agent of valuesOf(rows, "agent")) {
    if (agent.project_id && !retired.has(agent.owner_id)
      && ["coordinator", "worker"].includes(identities.get(agent.id)?.project_role)) selected.set(agent.project_id, agent.owner_id);
  }
  for (const owner of valuesOf(rows, "owner_session")) {
    if (owner.project_id && typeof owner.pinned_agent_id === "string") selected.set(owner.project_id, owner.owner_id);
  }
  return selected;
}

function heldProjectOwners(projects = [], retired) {
  const selected = new Map();
  for (const project of projects) {
    const owner = project.entity_id || project.run_id;
    if (owner && !retired.has(owner)) selected.set(project.id || project.project_id, owner);
  }
  return selected;
}

export function deviceDirectories(rows, held, deviceId) {
  const agents = valuesOf(rows, "agent"), sources = valuesOf(rows, "project_source"), directories = valuesOf(rows, "workspace_directory");
  const lifecycles = new Map(valuesOf(rows, "workspace_lifecycle").map((row) => [row.workspace_id, row]));
  const projectSessions = new Map(valuesOf(rows, "project_session").map((row) => [row.project_id, row]));
  const ownerRows = valuesOf(rows, "owner_session");
  const ownerSessions = new Map(ownerRows.filter((row) => row.workspace_id).map((row) => [row.workspace_id, row]));
  const projectOwnersById = projectOwners(rows, held);
  const projects = valuesOf(rows, "project").map((project) => stampProject({ ...project,
    ...sessionProjection(projectSessions.get(project.project_id)), entity_id: projectOwnersById.get(project.project_id) ?? null,
    sources: sources.filter((source) => source.project_id === project.project_id) }, deviceId));
  const workspaces = valuesOf(rows, "workspace").map((workspace) => stampWorkspace({ ...workspace,
    ...sessionProjection(ownerSessions.get(workspace.id)),
    lifecycle: lifecycleProjection(held("workspaces")?.find((row) => row.id === workspace.id)?.lifecycle, lifecycles.get(workspace.id)),
    entity_id: ownerIdOf(agents, "workspace_id", workspace.id) || ownerSessions.get(workspace.id)?.owner_id || workspace.entity_id,
    directories: directories.filter((directory) => directory.workspace_id === workspace.id) }, deviceId));
  const result = {
    projects: mergeList(held("projects"), projects, deletedKeys(rows, "project"), (project) => project.id || project.project_id),
    workspaces: mergeList(held("workspaces"), workspaces, deletedKeys(rows, "workspace"), (workspace) => workspace.id),
  };
  clearRetiredDirectoryFields(rows, result);
  return result;
}

const lifecycleFields = ["pruned_bytes", "pruned_at_ms", "noticed_at_ms", "size_bytes", "size_measured_at_ms"];
const sessionFields = ["session_started_ms", "last_activity_ms", "retained_last_activity_ms"];
const sessionProjection = (session) => Object.fromEntries(sessionFields.map((field) => [field, session?.[field] ?? null]));
const lifecycleProjection = (held, row) => row ? { ...held, ...Object.fromEntries(lifecycleFields.map((field) => [field, row[field]])) } : held;
const retiredRow = (record) => record.row || record.previousRow;
const clearFields = (row, fields) => { if (row) for (const field of fields) delete row[field]; };
function clearRetiredDirectoryFields(rows, result) {
  for (const record of [...rows.values()].filter((record) => record.deleted)) {
    const previous = retiredRow(record);
    if (record.kind === "workspace_lifecycle") clearFields(result.workspaces.find((row) => row.id === record.key)?.lifecycle, lifecycleFields);
    if (record.kind === "owner_session") clearFields(result.workspaces.find((row) => row.id === previous?.workspace_id), sessionFields);
    if (record.kind === "project_session") clearFields(result.projects.find((row) => row.id === record.key), sessionFields);
  }
}

function oldAgent(held, ownerId, id) {
  const row = held("row", ownerId), roster = held("project-agents", ownerId);
  const rowAgent = row?.agents?.find((agent) => agent.id === id), rosterAgent = roster?.agents?.find((agent) => agent.id === id);
  // Either immediate row repair or the complete roster's confirmation can
  // arrive first. Retain the stronger cursor on the same thread generation.
  if (!rowAgent) return rosterAgent;
  return rosterAgent ? preserveConversationRead(rowAgent, rosterAgent) : rowAgent;
}

function ownerRow(owner, agents, conversations, session, held, deviceId, projectOwnerIds) {
  const previous = held("row", owner), first = agents[0] || session;
  const summaries = agents.map((agent) => conversations.get(agent.id)).filter(Boolean);
  return stampRow({ ...previous, kind: ownerKind(owner, first, previous, projectOwnerIds), entity_id: owner,
    run_id: owner, project_id: first.project_id, workspace_id: first.workspace_id,
    agents, ...ownerAttention(agents),
    muted: summaries.length > 0 && summaries.every((row) => row.muted),
    dismissed: summaries.length > 0 && summaries.every((row) => row.dismissed),
    pending: summaries.some((row) => row.pending),
    ...conversationActivity(previous, summaries),
  }, deviceId);
}

function ownerKind(owner, first, previous, projectOwnerIds) {
  if (projectOwnerIds.has(owner)) return "project";
  if (first.workspace_id) return "branch";
  // A null workspace link is also normal for legacy plan/run owners. Keep a
  // known non-project kind; missing registry linkage never establishes one.
  return previous?.kind && previous.kind !== "project" ? previous.kind : undefined;
}

function ownerAttention(agents) {
  const watched = agents.filter((agent) => agent.watched !== false);
  const unread = watched.reduce((total, agent) => total + (agent.unread_count || 0), 0);
  return { unread: unread > 0, unread_count: unread, needs_attention: unread > 0,
    unread_reason: watched.find((agent) => agent.unread_count > 0)?.unread_reason ?? null };
}

function conversationActivity(previous, summaries) {
  const newest = summaries.reduce((held, row) => !held || row.last_activity_ms >= held.last_activity_ms ? row : held, null);
  return { title: newest?.title ?? previous?.title,
    last_message_summary: newest?.last_message_summary ?? previous?.last_message_summary };
}

export function deviceOwners(rows, held, deviceId) {
  const conversations = new Map(valuesOf(rows, "inbox_conversation").map((row) => [row.agent_id, row]));
  const identities = new Map(valuesOf(rows, "identity").map((row) => [row.agent_id, row]));
  const sessions = new Map(valuesOf(rows, "owner_session").map((row) => [row.owner_id, row]));
  const retired = deletedKeys(rows, "owner_session");
  const projectOwnerIds = new Set(projectOwners(rows, held).values());
  const owners = new Map();
  for (const agent of valuesOf(rows, "agent")) {
    if (retired.has(agent.owner_id)) continue;
    const next = agentProjection(agent, conversations.get(agent.id), identities.get(agent.id), oldAgent(held, agent.owner_id, agent.id));
    if (!owners.has(agent.owner_id)) owners.set(agent.owner_id, []);
    owners.get(agent.owner_id).push(next);
  }
  for (const owner of sessions.keys()) if (!owners.has(owner)) owners.set(owner, []);
  for (const agents of owners.values()) agents.sort((a, b) => a.ordinal - b.ordinal || a.id.localeCompare(b.id));
  return new Map([...owners].map(([owner, agents]) => {
    const session = sessions.get(owner);
    const row = ownerRow(owner, agents, conversations, session, held, deviceId, projectOwnerIds);
    // Unlinked legacy owners have no active rail consumer; retain raw facts
    // until a registered workspace, project authority or held kind identifies them.
    if (!row.kind) return null;
    Object.assign(row, sessionProjection(session));
    return [owner, row];
  }).filter(Boolean));
}

export function deviceListedOwners(owners, rosters) {
  return new Map([...owners].map(([owner, row]) => {
    const roster = rosters.get(owner);
    if (!roster) return [owner, row];
    // Legacy board digests list the project's pin alone; read-state repair
    // compares that exact roster. Workers remain in project-agents separately.
    const agents = row.agents.filter((agent) => agent.id === roster.pinned_agent_id);
    return [owner, { ...row, ...ownerAttention(agents), agents, pinned_agent_id: roster.pinned_agent_id }];
  }));
}

export function deviceProjectRosters(rows, owners, held) {
  const sessions = new Map(valuesOf(rows, "owner_session").map((row) => [row.owner_id, row]));
  const projectOwnerIds = new Set(projectOwners(rows, held).values());
  const rosters = new Map();
  for (const [owner, row] of owners) {
    if (!projectOwnerIds.has(owner)) continue;
    // The explicit pin and identity role are independent persisted facts;
    // neither a roster's order nor a missing agent can establish a coordinator.
    rosters.set(owner, { entity_id: owner, pinned_agent_id: sessions.get(owner)?.pinned_agent_id ?? null, agents: row.agents });
  }
  return rosters;
}

export function deviceFeed(rows, owners, directories, held, deviceId) {
  const previous = held("feed") || {};
  const captures = valuesOf(rows, "inbox_capture").map((row) => stampRow({ ...row, kind: "capture", capture_id: row.id,
    unread: row.needs_answer === true, unread_count: row.needs_answer ? 1 : 0,
    unread_reason: row.error ? "routing_failed" : null }, deviceId));
  const tasks = valuesOf(rows, "inbox_task").map((row) => stampRow({ ...row, kind: "tracker_task", task_id: row.id,
    unread: row.unread_count || 0 }, deviceId));
  const managed = new Set([...rows.values()].filter((entry) => ["inbox_task", "inbox_capture", "agent", "inbox_conversation", "owner_session"].includes(entry.kind))
    .map((entry) => (entry.row || entry.previousRow)?.owner_id || entry.key));
  const unchanged = (previous.items || []).filter((row) => !managed.has(row.entity_id || row.task_id || row.capture_id));
  const items = [...unchanged, ...owners.values(), ...captures, ...tasks];
  return { ...previous, ...directories, items, runs: [...owners.values()], cached: false };
}

export function deviceArchive(rows, previous = {}) {
  const incoming = valuesOf(rows, "archived_worktree").map((row) => ({ ...row, kind: "branch", worktree_id: row.id,
    title: row.name, state: row.status, finished_at: row.archived_at }));
  return { ...previous, items: mergeList(previous.items, incoming, deletedKeys(rows, "archived_worktree"), (row) => row.worktree_id || row.id) };
}

export const deviceFamilies = new Set(["project", "project_source", "workspace", "workspace_directory", "badge", "agent", "identity",
  "inbox_conversation", "inbox_task", "inbox_capture", "user_session", "archived_worktree", "workspace_lifecycle", "owner_session", "project_session"]);
// badge has no numeric directory-counter consumer; retain it as a raw record.
// identity is copied to each known project's existing summary-identity cache.
// owner_session/project_session supplement the existing owner/list session facts.
