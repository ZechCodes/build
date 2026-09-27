// What a workspace's Tasks face on its rail says: the unread of the watched
// tasks its agents hold, which is the badge (#104), and how many open tasks
// they hold, which is its tooltip.
//
// Counted over the project's cached list rather than a read per agent: the
// whole list is already on disk, one pass over it answers every agent in the
// workspace at once, and the badge moves straight off a pushed list with no
// read at all.
//
// This file used to hold the grouping the #16 overlay drew as well. The
// overlay is gone (#29) — the tasks are a tab of the workspace now, drawn by
// the tracker's own list and board — so what is left is the counts.
//
// No DOM, no app imports.

import { agentOpenTaskCount, assignedTo } from "./trackerAgentTasks.js";
import { watchedTasksUnread } from "./taskUnread.js";

/**
 * The agents of one workspace, as its board row carries them.
 *
 * A workspace with no conversation yet has no row and no agents, which is not
 * an error — it is a workspace nobody has spoken in, and it holds no tasks
 * for the same reason.
 */
export const workspaceAgentIds = (agents) => (agents || []).map((agent) => agent?.id).filter(Boolean);

/**
 * How many open tasks this workspace's agents are holding — the badge's
 * number.
 *
 * Open means not finished: neither closed nor sitting in Done. A badge is a
 * call to look, and finished work is not one. A task has exactly one
 * assignee, so summing per agent cannot double-count.
 */
export const workspaceOpenTaskCount = (tasks, agents) =>
  workspaceAgentIds(agents).reduce((total, agentId) => total + agentOpenTaskCount(tasks, agentId), 0);

/**
 * The unread of the watched tasks this workspace's agents hold — the badge's
 * number since #104: "for workspaces it is only unreads on tasks assigned to
 * an agent in that workspace". A task nobody watches counts nothing.
 */
export function workspaceTasksUnread(tasks, agents) {
  const agentIds = workspaceAgentIds(agents);
  return watchedTasksUnread(tasks, { only: (task) => agentIds.some((agentId) => assignedTo(task, agentId)) });
}

/** What the face says to a pointer: what is unread, then what is open. */
export function workspaceTasksTitle(unread, open) {
  const openWords = open ? `${open} open task${open === 1 ? "" : "s"} in this workspace` : "Tasks in this workspace";
  return unread ? `${unread} unread · ${openWords}` : openWords;
}
