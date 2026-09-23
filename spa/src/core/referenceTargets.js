// What a written reference names, out of the caches a surface already holds.
//
// #56 landed the two halves either side of this one: core/markdownRefs.js says
// what a reference IS (`#42`, `@workspace:build`, `@agent:…`, `[[ws:path#L10]]`)
// and core/markdownLinks.js says which route each kind opens. Between them sits
// the question neither can answer — WHICH issue is #42, which workspace is
// called build — and that is a question about this device, this project and
// what has been read here. This file answers it, and nothing else does.
//
// Every answer comes from a cache the surface is already painting from: the
// project's issue list, the feed's workspaces, and the agents the assignee
// picker names (core/trackerAssignee.js). Nothing here reads, fetches or
// awaits — a paint cannot — so a reference to something this client has never
// read stays the words the agent typed, which is the same thing that happens
// when there is no resolver at all. A link to nowhere is worse than prose.
//
// Pure: no DOM, no cache, no router.

import { workspaceDisplayName } from "./workspaceModel.js";
import { actorName } from "./trackerLineWords.js";

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
 * `issues` is the project's issue list as the tracker caches it, `workspaces`
 * the feed's for this project, and `agentGroups` what `workspaceAgents`
 * answers: the same three lists the issue page and the conversation already
 * paint from, so a link can only point at something the reader could have
 * opened anyway.
 */
export function referenceLinks({ place, issues = [], workspaces = [], agentGroups = [], identities = {} } = {}) {
  const deviceId = place?.deviceId;
  const projectId = place?.projectId;
  if (!deviceId || !projectId) return null;
  const at = { deviceId, projectId };
  return {
    issue(number) {
      const found = (issues || []).find((issue) => Number(issue.number) === Number(number));
      return found?.id ? { ...at, issueId: found.id, title: found.title || "" } : null;
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
