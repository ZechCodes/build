// What a task's links open.
//
// A task says what should be done; the doing is a workspace, a branch, a
// commit and a conversation, each of which already has a surface. So nothing
// here invents a destination — every row is a route the app already writes, or
// it is not a link at all.
//
// The one that takes a lookup is the conversation. `links.conversation_ids`
// holds conversation OWNER ids (`run-…`), and which page an owner belongs to is
// not in the id: a workspace's owner opens that workspace with the rail
// standing on it, and the project's own opens the project page. The feed knows
// which is which, so it is asked rather than guessed.
//
// A commit is the exception that proves the rule: there is no surface addressed
// by a bare hash, so a commit is shown and not linked. Drawing a dead link
// would be worse than drawing a fact.
//
// No DOM, no app imports.

import { conversationRoute } from "./router.js";
import { workspaceDisplayName } from "./workspaceModel.js";
import { taskLinks } from "./trackerModel.js";

/** How much of a hash is enough to recognize it, and the length git itself
 *  abbreviates to. */
const SHORT_HASH = 7;

const workspacesOfProject = (feed, projectKey) =>
  (feed?.workspaces || []).filter((workspace) => workspace.projectKey === projectKey);

const ownerOf = (workspace) => workspace.entity_id || workspace.run_id || workspace.id;

const workspaceRow = (workspaceId, place, feed, identities = {}) => {
  const workspace = workspacesOfProject(feed, place.projectKey).find(
    (candidate) => (candidate.workspace_id || candidate.id) === workspaceId,
  );
  return {
    kind: "workspace",
    workspaceId,
    label: workspace ? workspaceDisplayName(workspace)
      : Object.values(identities).find((identity) => identity.workspace_id === workspaceId)?.workspace_name || workspaceId,
    route: workspace ? { name: "workspace", projectId: place.projectId, deviceId: place.deviceId, workspaceId, tab: "changes" } : null,
  };
};

const branchRow = (branch, place) => ({
  kind: "branch",
  label: branch,
  route: { name: "branch", projectId: place.projectId, deviceId: place.deviceId, branch, tab: "changes" },
});

/** A commit is shown, not linked: no surface is addressed by a bare hash. */
const commitRow = (commit) => ({ kind: "commit", label: String(commit).slice(0, SHORT_HASH), title: commit, route: null });

const isProjectConversation = (conversationId, place, feed) => {
  const project = (feed?.projects || []).find((candidate) => candidate.projectKey === place.projectKey);
  return (project?.entity_id || project?.run_id) === conversationId;
};

/** Which page a conversation owner belongs to. A workspace of this project
 *  owns it, or the project itself does — and an owner the feed does not place
 *  opens the project page, which is where a conversation of the project's is
 *  always reachable. */
function conversationRow(conversationId, place, feed) {
  const workspace = workspacesOfProject(feed, place.projectKey).find(
    (candidate) => ownerOf(candidate) === conversationId,
  );
  const page = {
    kind: workspace ? "workspace" : "project",
    projectId: place.projectId,
    deviceId: place.deviceId,
    workspaceId: workspace ? workspace.workspace_id || workspace.id : null,
  };
  return {
    kind: "conversation",
    label: workspace ? `${workspaceDisplayName(workspace)} · conversation` : "Project agent · conversation",
    route: workspace || isProjectConversation(conversationId, place, feed) ? conversationRoute(page) : null,
  };
}

const parentRow = (parentTaskId, place) => ({
  kind: "parent",
  label: "Parent task",
  route: { name: "trackerTask", projectId: place.projectId, deviceId: place.deviceId, taskId: parentTaskId },
});

/**
 * Every link a task carries, in one list, in the order the record holds them.
 *
 * `place` is where the reader is standing — `{projectId, deviceId, projectKey}`
 * — because every route under a project is written against the machine that
 * project is on.
 */
export function taskLinkRows(task, place, feed = null) {
  const links = taskLinks(task);
  return [
    ...links.workspace_ids.map((workspaceId) => workspaceRow(workspaceId, place, feed, task.identities)),
    ...links.branches.map((branch) => branchRow(branch, place)),
    ...links.conversation_ids.map((conversationId) => conversationRow(conversationId, place, feed)),
    ...links.commits.map(commitRow),
    ...(links.parent_task_id ? [parentRow(links.parent_task_id, place)] : []),
  ];
}
