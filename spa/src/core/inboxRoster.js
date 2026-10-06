// What a conversation's roster says on an inbox row (#103): how many of its
// agents are running, and how much is waiting from the ones the user watches.
//
// A conversation is named by its project and its entity, and the board can
// carry its roster twice — on `runs`, which holds every run, and on `items`,
// which leaves out a run nobody watches — so the freshest copy is the one read.
//
// No DOM, no app imports.

import { entityIdOf } from "./entityId.js";
import { isAtLeastAsFresh } from "./cacheFreshness.js";
import { agentIsRunning } from "./agentRunning.js";

const conversationKey = (projectKey, entityId) => JSON.stringify([projectKey, entityId]);

/** The freshest row carrying a roster for each conversation the rows name:
 *  `(projectKey, entityId) => row | undefined`. */
export function freshestRosters(rows = []) {
  const byConversation = new Map();
  for (const row of rows) {
    const entityId = entityIdOf(row);
    if (!entityId || !Array.isArray(row.agents)) continue;
    const key = conversationKey(row.projectKey, entityId);
    const current = byConversation.get(key);
    if (!current || isAtLeastAsFresh(row, current)) byConversation.set(key, row);
  }
  return (projectKey, entityId) => byConversation.get(conversationKey(projectKey, entityId));
}

/** How many agents run, including those waiting on running descendants.
 * Each roster member counts once, however many descendants it has. */
export const runningAgentCount = (agents = []) => agents.filter(agentIsRunning).length;

/** The folded project summary includes only watched running agents. */
export const watchedAgentsRunning = (agents = []) => agents.some((agent) => agent.watched !== false && agentIsRunning(agent));

const unreadOf = (agents) => agents.reduce((total, agent) => total + (agent.unread_count || 0), 0);

/** Everything the watched agents are waiting to tell the user. An agent is
 *  watched unless the bridge says otherwise (#101). */
export const watchedUnreadCount = (agents = []) => unreadOf(agents.filter((agent) => agent.watched !== false));

/** Everything a conversation's agents are waiting to tell the user. */
export const agentsUnreadCount = (agents = []) => unreadOf(agents);
