// What a written reference names, out of the caches a surface already holds.
//
// #56 landed the two halves either side of this one: core/markdownRefs.js says
// what a reference IS (`#42`, `@workspace:build`, `@agent:…`, `[[ws:path#L10]]`)
// and core/markdownLinks.js says which route each kind opens. Between them sits
// the question neither can answer — WHICH task is #42, which workspace is
// called build — and that is a question about this device, this project and
// what has been read here. This file answers it, and nothing else does.
//
// Every answer comes from a cache the surface is already painting from: the
// project's task list, the feed's workspaces, and the agents the assignee
// picker names (core/trackerAssignee.js). Nothing here reads, fetches or
// awaits — a paint cannot — so a reference to something this client has never
// read stays the words the agent typed, which is the same thing that happens
// when there is no resolver at all. A link to nowhere is worse than prose.
//
// Pure: no DOM, no cache, no router.

import { workspaceDisplayName } from "./workspaceModel.js";
import { actorName } from "./trackerLineWords.js";
import { workspaceAgents } from "./trackerAssignee.js";
import { deviceKey } from "./deviceKey.js";

/** The id a project's own agent is minted under — it stands in no workspace,
 *  so it is reached on the project's own page (core/markdownLinks.js). */
const PROJECT_AGENT_PREFIX = "project-";

const sameText = (left, right) =>
  String(left || "").trim().toLowerCase() === String(right || "").trim().toLowerCase();

/** A workspace by the name a reader would write, or by its id — an agent that
 *  knows only the id should not have to learn the name to point at it. */
const workspaceNamed = (workspaces, said) =>
  (workspaces || []).find((workspace) => {
    const id = workspace.workspace_id || workspace.id;
    return id === said || sameText(workspaceDisplayName(workspace), said);
  }) || null;

/** The workspace one agent stands in, out of the picker's own grouping. */
const groupHolding = (groups, agentId) =>
  (groups || []).find((group) => (group.agents || []).some((agent) => agent.id === agentId)) || null;

const identityAgent = (id, identity, identities, workspaces, at) => {
  const workspaceId = identity.workspace_id;
  if (!identity.available || !workspaceId || !workspaces.some((workspace) =>
    (workspace.workspace_id || workspace.id) === workspaceId)) return null;
  return { ...at, workspaceId, agentId: id,
    name: actorName({ kind: "agent", agent_id: id }, { identities }) };
};

/**
 * The resolver core/markdownLinks.js asks, built from what this surface holds.
 *
 * `place` is where the reader is — the machine and the project every route is
 * written from. Without both there is nowhere to send anyone, so this answers
 * null and the renderer leaves every reference as prose.
 *
 * `tasks` is the project's task list as the tracker caches it, `workspaces`
 * the feed's for this project, and `agentGroups` what `workspaceAgents`
 * answers: the same three lists the task page and the conversation already
 * paint from, so a link can only point at something the reader could have
 * opened anyway.
 */
export function referenceLinks({ place, tasks = [], workspaces = [], agentGroups = [], identities = {} } = {}) {
  const deviceId = place?.deviceId;
  const projectId = place?.projectId;
  if (!deviceId || !projectId) return null;
  const at = { deviceId, projectId };
  return {
    task(number) {
      const found = (tasks || []).find((task) => Number(task.number) === Number(number));
      return found?.id ? { ...at, taskId: found.id, title: found.title || "" } : null;
    },

    workspace(said) {
      const found = workspaceNamed(workspaces, said);
      if (!found) return null;
      return { ...at, workspaceId: found.workspace_id || found.id, name: workspaceDisplayName(found) };
    },

    agent(id) {
      if (String(id || "").startsWith(PROJECT_AGENT_PREFIX)) {
        return identities[id]?.available === false ? null : { ...at, agentId: id };
      }
      const identity = identities[id];
      if (identity) return identityAgent(id, identity, identities, workspaces, at);
      const group = groupHolding(agentGroups, id);
      if (!group) return null;
      const agent = group.agents.find((one) => one.id === id);
      return { ...at, workspaceId: group.workspaceId, agentId: id, name: agent?.label || "" };
    },
  };
}

// ─── The whole account's index (#229) ────────────────────────────────────────
//
// `referenceLinks` above answers for the one project a surface stands in, out
// of the lists that surface holds. What follows answers for everything this
// client has read — every machine, every project — so a reference to another
// project's workspace resolves too, and every surface answers the same way
// because they all ask the same index (core/referenceIndex.js). It is still
// only caches: the feed, and the task lists the tracker has written.
//
// Each answer is one of three things. A place is a link. `null` means the list
// that would hold it is held and it is not in it: the reader is told so
// (core/markdownLinks.js). `undefined` means nothing here can tell — no
// project to read `#42` in, a task list never read — and the reference is left
// as the words that were typed, exactly as before there was an index.

const idOf = (row) => row.workspace_id || row.id;

/** The (device, project) a key names, as a route writes it. */
const placeOfKey = (projectKey) => {
  const seam = String(projectKey || "").indexOf("/");
  return { deviceId: projectKey.slice(0, seam), projectId: projectKey.slice(seam + 1) };
};

/** One source directory of a workspace, by the id a route carries and the
 *  name a reader writes. */
const directoryOf = (directory) => ({ sourceId: directory.source_id || "", name: directory.name || "" });

/** Where a feed row lives: the machine and project it was stamped with
 *  (core/feedMerge.js), or else the halves of its account-wide key. */
const placeOfRow = (row) =>
  row.deviceId && row.project_id ? { deviceId: row.deviceId, projectId: row.project_id } : placeOfKey(row.projectKey);

/** One workspace, as every answer about it reads. */
const workspaceEntry = (workspace) => ({
  ...placeOfRow(workspace),
  projectKey: workspace.projectKey,
  workspaceId: idOf(workspace),
  name: workspaceDisplayName(workspace),
  directories: (workspace.directories || []).map(directoryOf).filter((directory) => directory.sourceId),
});

/** Every agent standing in a workspace, by id, out of the feed's agent digests
 *  — the same grouping and labels the assignee picker and the rail use. */
function agentsOf(feed, workspaces) {
  const agents = new Map();
  const byWorkspace = new Map(workspaces.map((workspace) => [workspace.workspaceId, workspace]));
  const projectKeys = new Set(workspaces.map((workspace) => workspace.projectKey));
  for (const projectKey of projectKeys) {
    for (const group of workspaceAgents(feed, projectKey)) {
      const workspace = byWorkspace.get(group.workspaceId);
      for (const agent of group.agents) agents.set(agent.id, { workspace, name: agent.label });
    }
  }
  return agents;
}

/**
 * What the index is built from, laid out once for every question asked of it.
 *
 * `feed` is the merge every surface reads (core/taskFeed.js). `tasks` is each
 * project's task list by its account-wide key, as core/trackerCache.js holds it.
 * Built once per change to either, so a paint that renders a hundred messages
 * asks a hundred cheap questions rather than rebuilding a hundred indexes.
 */
export function referenceTables({ feed = null, tasks = {} } = {}) {
  const workspaces = (feed?.workspaces || []).filter((workspace) => workspace.projectKey).map(workspaceEntry);
  const projects = (feed?.projects || []).filter((project) => project.projectKey).map((project) => ({
    ...placeOfKey(project.projectKey), projectKey: project.projectKey, name: String(project.name || project.id || ""),
  }));
  return { workspaces, projects, agents: agentsOf(feed, workspaces), tasks: tasks || {}, held: Boolean(feed) };
}

/** The one row a reader meant, out of those that answer to what they wrote:
 *  the reader's own project first, then the rest of the account where the
 *  answer is unambiguous. Two workspaces called "fixes" in two other projects
 *  is a guess, and a guess is not a link. */
function nearest(rows, projectKey) {
  const here = rows.find((row) => row.projectKey === projectKey);
  if (here) return here;
  return rows.length === 1 ? rows[0] : null;
}

/** Rows that answer to an id exactly, or else to a name in any case. */
function answering(rows, said, idField) {
  const byId = rows.filter((row) => row[idField] === said);
  return byId.length ? byId : rows.filter((row) => sameText(row.name, said));
}

/** A directory of one workspace by its name or its source id. */
const directoryNamed = (workspace, said) =>
  workspace.directories.find((directory) => directory.sourceId === said || sameText(directory.name, said)) || null;

/** `<workspace>` or `<workspace>/<directory>`: the whole text as a workspace's
 *  name first, since a name may hold a slash, and only then split at the last
 *  slash into a workspace and one of its directories. */
function workspaceAt(tables, said, projectKey) {
  const whole = nearest(answering(tables.workspaces, said, "workspaceId"), projectKey);
  if (whole) return whole;
  const seam = said.lastIndexOf("/");
  if (seam <= 0) return null;
  const workspace = nearest(answering(tables.workspaces, said.slice(0, seam).trim(), "workspaceId"), projectKey);
  const directory = workspace && directoryNamed(workspace, said.slice(seam + 1).trim());
  return directory ? { ...workspace, sourceId: directory.sourceId } : null;
}

/** An agent a surface knows by its identity (a task carries its actors'),
 *  where the workspace it names is one the index holds. */
function identified(tables, id, identity) {
  if (!identity?.available || !identity.workspace_id) return null;
  const workspace = tables.workspaces.find((row) => row.workspaceId === identity.workspace_id);
  return workspace ? { workspace, name: identity.name || "" } : null;
}

/** A project's own agent: it stands in no workspace, so it is reached on its
 *  project's page — the reader's project, since its id names no other. With
 *  no project to stand in, nothing can say which page that is. */
function projectAgent(tables, id, { place, identities }) {
  if (!place) return undefined;
  if (identities[id]?.available === false) return null;
  const project = tables.projects.find((row) => row.projectKey === place.projectKey);
  return { deviceId: place.deviceId, projectId: place.projectId, agentId: id, name: project ? `${project.name} agent` : "" };
}

/**
 * The resolver core/markdownLinks.js asks, over the tables above, for a reader
 * standing at `place` (`{ deviceId, projectId }`, or nothing).
 *
 * `identities` is what a surface knows about agents beyond the feed — a task
 * carries every actor on it, departed ones included — and is asked first.
 */
export function resolverFor(tables, { place = null, identities = {} } = {}) {
  const here = place?.deviceId && place?.projectId
    ? { deviceId: place.deviceId, projectId: place.projectId, projectKey: deviceKey(place.deviceId, place.projectId) }
    : null;
  const known = identities || {};
  const heldOrUnknown = (answer) => (tables.held ? answer : undefined);
  return {
    task(number) {
      const list = here && tables.tasks[here.projectKey];
      if (!Array.isArray(list)) return undefined;
      const found = list.find((task) => Number(task.number) === Number(number));
      return found?.id ? { deviceId: here.deviceId, projectId: here.projectId, taskId: found.id, title: found.title || "" } : null;
    },
    workspace: (said) => heldOrUnknown(workspaceAt(tables, String(said || "").trim(), here?.projectKey)),
    project: (said) => heldOrUnknown(nearest(answering(tables.projects, String(said || "").trim(), "projectId"), here?.projectKey)),
    agent(id) {
      if (String(id || "").startsWith(PROJECT_AGENT_PREFIX)) return heldOrUnknown(projectAgent(tables, id, { place: here, identities: known }));
      const standing = identified(tables, id, known[id]) || tables.agents.get(id);
      if (!standing) return heldOrUnknown(null);
      const { workspace, name } = standing;
      return { deviceId: workspace.deviceId, projectId: workspace.projectId, workspaceId: workspace.workspaceId, agentId: id, name };
    },
  };
}
