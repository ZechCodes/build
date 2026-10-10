// Every conversation badge reads the user's watch before its cached count.
// Older bridges omit watching; only an explicit false suppresses their unread.

export const conversationIsWatched = (conversation) => conversation?.watched !== false;

export function conversationUnreadCount(conversation) {
  if (!conversationIsWatched(conversation)) return 0;
  return Math.max(0, Number(conversation?.unread_count) || 0);
}

export const rosterUnreadCount = (agents = []) => agents.reduce(
  (total, agent) => total + conversationUnreadCount(agent), 0,
);

/** A populated roster is the authority; an older summary stands in when
 *  the bridge carries no agents. A bare unread flag may count as one for menus. */
export function conversationSummaryUnreadCount(row, flagFallback = false) {
  if (!conversationIsWatched(row)) return 0;
  if (Array.isArray(row?.agents) && row.agents.length) return rosterUnreadCount(row.agents);
  const count = conversationUnreadCount(row);
  return count || (flagFallback && row?.unread ? 1 : 0);
}
