// Descendant activity on one device's feed snapshot, derived from its cached
// rosters. The snapshot stays separate from the replicas it was read from.
import { agentLineage, lineageMembers, withRollup } from "./agentLineageModel.js";
import { entityIdOf } from "./entityId.js";
import { freshestRosters } from "./inboxRoster.js";
import { projectRosterRows } from "./projectRosterFeed.js";
import { conversationUnreadCount, withConversationReader } from "./conversationUnread.js";

/** Only the canonical lineage's watched conversations can add unread. This
 * indicator travels up the cut forest; numeric counts stay with their owners. */
function withUnread(lineage) {
  const unread = new Map();
  const descendantsUnread = (agentId) => {
    if (!unread.has(agentId)) {
      unread.set(agentId, lineage.createdBy(agentId).some(({ agent }) =>
        conversationUnreadCount(agent) > 0 || descendantsUnread(agent.id)));
    }
    return unread.get(agentId);
  };
  return { ...lineage, descendantsUnread };
}

function withFeedRollup(agent, lineage) {
  const running = withRollup(agent, lineage);
  return lineage?.descendantsUnread(agent.id) ? { ...running, agents_unread: true } : running;
}

/** Use each conversation's newest roster, including runs absent from items. */
function newestRosters(rows) {
  const rosterOf = freshestRosters(rows);
  const named = new Map(rows.filter((row) => entityIdOf(row)).map((row) => [entityIdOf(row), row]));
  return [...named.values()].map((row) => rosterOf(row.projectKey, entityIdOf(row))).filter(Boolean);
}

/** Each project gets its own lineage: an id or creator in another project
 * cannot make this project's parent run. The caller already scopes devices. */
function projectLineages(snapshot, namesMakers) {
  const rows = projectRosterRows({ ...snapshot, items: [...(snapshot.runs || []), ...snapshot.items] });
  const projectIds = new Set(rows.map((row) => row.project_id));
  return new Map([...projectIds].map((projectId) => {
    const members = lineageMembers(newestRosters(rows.filter((row) => row.project_id === projectId)), {
      workspaces: snapshot.workspaces, projects: snapshot.projects,
    });
    return [projectId, withUnread(agentLineage(members, { namesMakers }))];
  }));
}

/** Roster members already represented on another member's panel contribute
 * there once. The lineage has cut maker cycles and respects cached support. */
function rosterDescendants(agents, lineage) {
  const descendants = new Set();
  const pending = agents.flatMap((agent) => lineage.createdBy(agent.id));
  for (const member of pending) {
    if (descendants.has(member.agent.id)) continue;
    descendants.add(member.agent.id);
    pending.push(...lineage.createdBy(member.agent.id));
  }
  return descendants;
}

/** Count each project's independent agent's own loop and running panel rows.
 * A waiting pin with three running workers therefore contributes three. */
function projectRunningCount(agents, lineage) {
  const conversations = [...new Map(agents.map((agent) => [agent.conversation_id || agent.id, agent])).values()];
  const descendants = rosterDescendants(conversations, lineage);
  return conversations.filter((agent) => !descendants.has(agent.id))
    .reduce((count, agent) => count + Number(!!agent.working) + (agent.agents_running || 0), 0);
}

/** Add activity rollups without changing each agent's own working flag or
 * unread count, or writing derived digests to disk. Spreads carry freshness. */
export function withFeedAgentRollups(snapshot, { namesMakers = false, isBeingRead = () => false } = {}) {
  if (!snapshot) return snapshot;
  snapshot = withReaders(snapshot, isBeingRead);
  const lineages = projectLineages(snapshot, namesMakers);
  const decorate = (row) => Array.isArray(row.agents)
    ? { ...row, agents: row.agents.map((agent) => withFeedRollup(agent, lineages.get(row.project_id))) } : row;
  const projects = (snapshot.projects || []).map((project) => {
    if (!project.projectRoster) return project;
    const lineage = lineages.get(project.id || project.project_id);
    const agents = project.projectRoster.agents.map((agent) => withFeedRollup(agent, lineage));
    return { ...project, projectRoster: { ...project.projectRoster, agents,
      running_count: projectRunningCount(agents, lineage) } };
  });
  return { ...snapshot, projects, items: snapshot.items.map(decorate), runs: (snapshot.runs || []).map(decorate) };
}

/** Reader presence is applied before descendant totals, on the derived copy
 * alone. Both complete project rosters and ordinary owner rows use it. */
function withReaders(snapshot, isBeingRead) {
  const roster = (row) => {
    const entityId = entityIdOf(row);
    if (!Array.isArray(row.agents)) return row;
    return { ...row, agents: row.agents.map((agent) => withConversationReader(agent, isBeingRead({
      entityId, agentId: agent.id, conversationId: agent.conversation_id, threadId: agent.thread_id,
    }))) };
  };
  return { ...snapshot, items: snapshot.items.map(roster), runs: (snapshot.runs || []).map(roster),
    projects: (snapshot.projects || []).map((project) => project.projectRoster
      ? { ...project, projectRoster: roster(project.projectRoster) } : project) };
}
