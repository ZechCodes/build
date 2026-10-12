// Destination badges and rail rows share watched conversation/task ownership.
// A task contributes to its workspace, or to the project when no visible
// workspace owns it; the project heading totals those contributions once.
import { watchedWorkspaceEntries } from "./inbox.js";
import { projectAgentEntries } from "./inboxProjectAgent.js";
import { workspaceProjectBlocks } from "./inboxProjects.js";
import { NO_TASK_UNREAD } from "./taskUnread.js";

export function navigationUnreadEntries({ projects = [], workspaces = [], items = [], runs = [], taskUnread = NO_TASK_UNREAD } = {}) {
  const workspaceRows = watchedWorkspaceEntries(workspaces, projects, items, runs, taskUnread);
  return [
    ...workspaceRows,
    ...projectAgentEntries(projects, items, runs, taskUnread.unheldBy(workspaceRows)),
  ];
}

/** The complete roster can contain unwatched workspaces absent from the rail.
 * Those remain destinations with zero unread, without changing rail policy. */
export function navigationUnreadCounts(input = {}) {
  const entries = navigationUnreadEntries(input);
  const { blocks } = workspaceProjectBlocks(entries, input.projects);
  const workspaces = new Map((input.workspaces || []).map((workspace) => [workspace.workspaceKey, 0]));
  for (const entry of entries) {
    if (entry.kind === "workspace") workspaces.set(entry.workspaceKey, entry.unreadCount);
  }
  return {
    projects: new Map(blocks.map((block) => [block.projectKey, block.unreadCount])),
    projectOwn: new Map(blocks.map((block) => [block.projectKey, block.agentEntry?.unreadCount || 0])),
    workspaces,
  };
}
