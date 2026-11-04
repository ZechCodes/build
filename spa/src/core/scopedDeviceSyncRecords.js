// Device rows are durable summaries, never live process or filesystem state.
// Rebuild the existing cache projections; mounted views never read this wire.
import { stampProject, stampRow, stampWorkspace } from "./feedMerge.js";
import { deviceRuntimeRowFields } from "./scopedDeviceFields.js";

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
  const base = sameThread(previous, agent) ? previous : undefined;
  const incoming = { ...base, ...agent, ...conversation, id: agent.id,
    conversation_id: agent.conversation_id, project_role: identity?.project_role ?? null };
  return preserveConversationRead(previous, incoming);
}

const ownerIdOf = (agents, field, id) => agents.find((agent) => agent[field] === id)?.owner_id;
const mergeList = (held, incoming, deleted, idOf, snapshot = false) => {
  const present = new Set(incoming.map(idOf));
  const next = new Map((held || []).filter((row) => !deleted.has(idOf(row)) && (!snapshot || present.has(idOf(row)))).map((row) => [idOf(row), row]));
  for (const row of incoming) next.set(idOf(row), { ...next.get(idOf(row)), ...row });
  return [...next.values()];
};
const deletedKeys = (rows, kind) => new Set([...rows.values()].filter((entry) => entry.kind === kind && entry.deleted).map((entry) => entry.key));

function projectOwners(rows, held, snapshot = false) {
  const retired = deletedKeys(rows, "owner_session");
  const selected = heldProjectOwners(held("projects"), retired);
  if (snapshot) removeAbsentOwnerHints(rows, selected);
  const identities = new Map(valuesOf(rows, "identity").map((row) => [row.agent_id, row]));
  selectRoleOwners(rows, selected, identities, retired);
  for (const owner of valuesOf(rows, "owner_session")) {
    if (owner.project_id && typeof owner.pinned_agent_id === "string") selected.set(owner.project_id, owner.owner_id);
  }
  return selected;
}

function removeAbsentOwnerHints(rows, selected) {
  const present = new Set([...valuesOf(rows, "agent"), ...valuesOf(rows, "owner_session")].map((row) => row.owner_id));
  for (const [project, owner] of selected) if (!present.has(owner)) selected.delete(project);
}

function selectRoleOwners(rows, selected, identities, retired) {
  for (const agent of valuesOf(rows, "agent")) {
    if (agent.project_id && !retired.has(agent.owner_id)
      && ["coordinator", "worker"].includes(identities.get(agent.id)?.project_role)) selected.set(agent.project_id, agent.owner_id);
  }
}

function heldProjectOwners(projects = [], retired) {
  const selected = new Map();
  for (const project of projects) {
    const owner = project.entity_id || project.run_id;
    if (owner && !retired.has(owner)) selected.set(project.id || project.project_id, owner);
  }
  return selected;
}

export function deviceDirectories(rows, held, deviceId, snapshot = false) {
  const agents = valuesOf(rows, "agent"), sources = valuesOf(rows, "project_source"), directories = valuesOf(rows, "workspace_directory");
  const lifecycles = new Map(valuesOf(rows, "workspace_lifecycle").map((row) => [row.workspace_id, row]));
  const projectSessions = new Map(valuesOf(rows, "project_session").map((row) => [row.project_id, row]));
  const ownerRows = valuesOf(rows, "owner_session");
  const ownerSessions = new Map(ownerRows.filter((row) => row.workspace_id).map((row) => [row.workspace_id, row]));
  const projectOwnersById = projectOwners(rows, held, snapshot);
  // The registry identifies the primary source by its exact project path.
  const projects = valuesOf(rows, "project").map((project) => stampProject({ ...project,
    is_git: sources.find((source) => source.project_id === project.project_id && source.path === project.path)?.is_git,
    ...sessionProjection(projectSessions.get(project.project_id)), entity_id: projectOwnersById.get(project.project_id) ?? null,
    sources: sources.filter((source) => source.project_id === project.project_id) }, deviceId));
  const workspaces = valuesOf(rows, "workspace").map((workspace) => stampWorkspace({ ...workspace,
    ...sessionProjection(ownerSessions.get(workspace.id)),
    lifecycle: lifecycleProjection(held("workspaces")?.find((row) => row.id === workspace.id)?.lifecycle, lifecycles.get(workspace.id)),
    entity_id: ownerIdOf(agents, "workspace_id", workspace.id) || ownerSessions.get(workspace.id)?.owner_id || workspace.entity_id,
    directories: directories.filter((directory) => directory.workspace_id === workspace.id) }, deviceId));
  const result = {
    projects: mergeList(held("projects"), projects, deletedKeys(rows, "project"), (project) => project.id || project.project_id, snapshot),
    workspaces: mergeList(held("workspaces"), workspaces, deletedKeys(rows, "workspace"), (workspace) => workspace.id, snapshot),
  };
  clearRetiredDirectoryFields(rows, result);
  return result;
}

export function deviceProjectReplica(rows, project, held) {
  const source = rows.get(deviceRecordId("project", project.id))?.row;
  if (!source) return held || project;
  // Each projection retains its own runtime/policy observations; copying a
  // sibling list's held fields could overwrite a newer settings observation.
  const next = { ...held, ...source };
  for (const field of ["id", "deviceId", "projectKey", "sources", "entity_id", "is_git", ...sessionFields]) {
    if (Object.hasOwn(project, field)) next[field] = project[field];
    else delete next[field];
  }
  return next;
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

function oldAgent(held, ownerId, id, current) {
  const row = held("row", ownerId), roster = held("project-agents", ownerId);
  const rowAgent = row?.agents?.find((agent) => agent.id === id), rosterAgent = roster?.agents?.find((agent) => agent.id === id);
  // Either immediate row repair or the complete roster's confirmation can
  // arrive first. Retain the stronger cursor on the same thread generation.
  if (!rowAgent) return rosterAgent;
  if (!rosterAgent || sameThread(rowAgent, current) && !sameThread(rosterAgent, current)) return rowAgent;
  return preserveConversationRead(rowAgent, rosterAgent);
}

function ownerRow(owner, agents, conversations, session, held, deviceId, projectOwnerIds, retiredConversations) {
  const previous = held("row", owner), first = agents[0] || session;
  const listed = projectOwnerIds.has(owner) ? agents.filter((agent) => agent.id === session?.pinned_agent_id) : agents;
  const summaries = listed.map((agent) => conversations.get(agent.id)).filter(Boolean);
  return stampRow({ ...previous, kind: ownerKind(owner, first, previous, projectOwnerIds), entity_id: owner,
    run_id: owner, project_id: first.project_id, workspace_id: first.workspace_id,
    agents, ...ownerAttention(agents),
    muted: summaries.length > 0 && summaries.every((row) => row.muted),
    dismissed: summaries.length > 0 && summaries.every((row) => row.dismissed),
    pending: summaries.some((row) => row.pending),
    ...conversationActivity(previousConversation(previous, listed, retiredConversations), summaries),
  }, deviceId);
}

function previousConversation(previous, agents, retired) {
  if (!previous) return undefined;
  const before = new Map((previous.agents || []).map((agent) => [agent.id, agent]));
  if (before.size !== agents.length) return undefined;
  return agents.every((agent) => sameThread(before.get(agent.id), agent)
    && before.get(agent.id).watched === agent.watched && !retired.has(agent.id)) ? previous : undefined;
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
  return { title: observedField(newest, previous, "title"),
    last_message_summary: observedField(newest, previous, "last_message_summary") ?? null };
}

const observedField = (incoming, previous, field) => incoming && Object.hasOwn(incoming, field) ? incoming[field] : previous?.[field];

export function deviceConversationRecords(rows) {
  return [...rows.values()].filter((record) => record.kind === "inbox_conversation" && !record.deleted
    && sameThread(rows.get(deviceRecordId("agent", record.row.agent_id))?.row, record.row));
}

export function devicePendingConversationOwners(rows) {
  return new Set([...rows.values()].filter((record) => record.kind === "inbox_conversation" && !record.deleted)
    .filter((record) => {
      const agent = rows.get(deviceRecordId("agent", record.row.agent_id))?.row;
      return agent && !sameThread(agent, record.row);
    }).map((record) => record.row.owner_id));
}

export function deviceOwners(rows, held, deviceId) {
  // A change page may split an agent reset from its conversation reset. Never
  // attach the older conversation generation to that newly observed agent.
  const conversations = new Map(deviceConversationRecords(rows).map(({ row }) => [row.agent_id, row]));
  const identities = new Map(valuesOf(rows, "identity").map((row) => [row.agent_id, row]));
  const sessions = new Map(valuesOf(rows, "owner_session").map((row) => [row.owner_id, row]));
  const retired = deletedKeys(rows, "owner_session");
  const retiredConversations = deletedKeys(rows, "inbox_conversation");
  const projectOwnerIds = new Set(projectOwners(rows, held).values());
  const owners = new Map();
  for (const agent of valuesOf(rows, "agent")) {
    if (retired.has(agent.owner_id)) continue;
    const next = agentProjection(agent, conversations.get(agent.id), identities.get(agent.id), oldAgent(held, agent.owner_id, agent.id, agent));
    if (!owners.has(agent.owner_id)) owners.set(agent.owner_id, []);
    owners.get(agent.owner_id).push(next);
  }
  for (const owner of sessions.keys()) if (!owners.has(owner)) owners.set(owner, []);
  for (const agents of owners.values()) agents.sort((a, b) => a.ordinal - b.ordinal || a.id.localeCompare(b.id));
  return new Map([...owners].map(([owner, agents]) => {
    const session = sessions.get(owner);
    const row = ownerRow(owner, agents, conversations, session, held, deviceId, projectOwnerIds, retiredConversations);
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

export function deviceFeed(rows, owners, directories, held, deviceId, snapshot = false) {
  const previous = held("feed") || {};
  const captures = valuesOf(rows, "inbox_capture").map((row) => stampRow({ ...row, kind: "capture", capture_id: row.id,
    working: row.state === "routing", unread: row.needs_answer === true, unread_count: row.needs_answer ? 1 : 0,
    unread_reason: row.error ? "routing_failed" : null }, deviceId));
  const tasks = valuesOf(rows, "inbox_task").map((row) => stampRow({ ...row, kind: "tracker_task", task_id: row.id,
    unread: row.unread_count || 0 }, deviceId));
  const managed = new Set([...rows.values()].filter((entry) => ["inbox_task", "inbox_capture", "agent", "inbox_conversation", "owner_session"].includes(entry.kind))
    .map((entry) => (entry.row || entry.previousRow)?.owner_id || entry.key));
  const unchanged = snapshot ? [] : (previous.items || []).filter((row) => !managed.has(row.entity_id || row.task_id || row.capture_id));
  const items = [...unchanged, ...owners.values(), ...captures, ...tasks];
  // Pending verbs, external checkouts, plans and full run bodies remain live
  // board observations; device families own only items and directory membership.
  return { ...previous, projects: directoryRuntimeAliases(directories.projects, previous.projects),
    workspaces: directoryRuntimeAliases(directories.workspaces, previous.workspaces), items, cached: false };
}

function directoryRuntimeAliases(rows, previous) {
  const before = new Map((previous || []).map((row) => [row.id, row]));
  // State enrichment also observes directory aliases inside the feed without
  // rewriting their canonical lists. Preserve that alias's runtime fields.
  return rows.map((row) => ({ ...row, ...Object.fromEntries(deviceRuntimeRowFields
    .filter((field) => Object.hasOwn(before.get(row.id) || {}, field)).map((field) => [field, before.get(row.id)[field]])) }));
}

const standaloneWorktree = (row) => Boolean(row.worktree_id) && !row.run_id && !row.task_id && !row.workspace_id;
export function deviceArchive(rows, previous = {}, snapshot = false) {
  const incoming = valuesOf(rows, "archived_worktree").map((row) => ({ ...row, kind: "branch", worktree_id: row.id,
    title: row.name, state: row.status, finished_at: row.archived_at }));
  // archived.list also owns historical runs/plans and finished workspaces.
  // Device membership covers standalone archived worktrees, not that composite.
  const outside = (previous.items || []).filter((row) => !standaloneWorktree(row));
  const represented = new Set(outside.map((row) => row.worktree_id).filter(Boolean));
  const selected = incoming.filter((row) => !represented.has(row.worktree_id));
  const worktrees = (previous.items || []).filter(standaloneWorktree);
  return { ...previous, items: [...outside,
    ...mergeList(worktrees, selected, deletedKeys(rows, "archived_worktree"), (row) => row.worktree_id || row.id, snapshot)] };
}

export const deviceFamilies = new Set(["project", "project_source", "workspace", "workspace_directory", "badge", "agent", "identity",
  "inbox_conversation", "inbox_task", "inbox_capture", "user_session", "archived_worktree", "workspace_lifecycle", "owner_session", "project_session"]);
// badge has no numeric directory-counter consumer; retain it as a raw record.
// identity is copied to each known project's existing summary-identity cache.
// owner_session/project_session supplement the existing owner/list session facts.
