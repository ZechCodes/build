// What a workspace's agents are carrying on the issue board.
//
// The same four questions core/trackerAgentIssues.js asks of one agent, asked
// of each agent in a workspace — minus the fourth. An agent's own conversation
// shows what it is TRACKING because that is context for reading what it says;
// a workspace's issues view is about what the work here is, and an issue
// somebody else holds is not that however many agents are following it.
//
// The filter is over the project's cached list rather than a read per agent:
// the whole list is already on disk, one pass over it answers every agent in
// the workspace at once, and it repaints straight off a pushed list with no
// read at all.
//
// No DOM, no app imports.

import { WATCHING_GROUP, agentIssueGroups, agentOpenIssueCount } from "./trackerAgentIssues.js";

/**
 * The agents of one workspace, as its board row carries them.
 *
 * A workspace with no conversation yet has no row and no agents, which is not
 * an error — it is a workspace nobody has spoken in, and it holds no issues
 * for the same reason.
 */
export const workspaceAgentIds = (agents) => (agents || []).map((agent) => agent?.id).filter(Boolean);

/** What an agent is called in a list of its workspace's agents. An agent has no
 *  name of its own, only a place on the strip, so that place is the name —
 *  matching the bubbles above it in the rail. */
export const agentLabel = (agent, index) => `Agent ${agent?.ordinal || index + 1}`;

/**
 * Every issue assigned to an agent of this workspace, grouped by agent.
 *
 * Agents keep the order the row lists them in, so the sections read down the
 * page in the same order the rail's bubbles read across. An agent holding
 * nothing is left out entirely rather than given an empty section: the view
 * answers "what is being worked here", and an agent with nothing to show is
 * not part of that answer.
 */
export function workspaceIssueGroups(issues, agents) {
  return (agents || [])
    .map((agent, index) => ({
      agentId: agent?.id || "",
      label: agentLabel(agent, index),
      // Assigned only. `agentIssueGroups` also answers what the agent merely
      // follows, which belongs in its conversation and not here.
      groups: agentIssueGroups(issues, agent?.id).filter((group) => group.id !== WATCHING_GROUP),
    }))
    .filter((section) => section.agentId && section.groups.length);
}

/**
 * How many open issues this workspace's agents are holding — the badge's
 * number.
 *
 * Open means not finished: neither closed nor sitting in Done. A badge is a
 * call to look, and finished work is not one. An issue has exactly one
 * assignee, so summing per agent cannot double-count.
 */
export const workspaceOpenIssueCount = (issues, agents) =>
  workspaceAgentIds(agents).reduce((total, agentId) => total + agentOpenIssueCount(issues, agentId), 0);
