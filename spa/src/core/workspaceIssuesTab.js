// The workspace's Issues tab: the project's tracker, narrowed to the issues the
// agents standing in THIS workspace are holding (#29).
//
// It replaces the overlay the icon beside the cog used to open. An overlay was
// the wrong shape for what the reader wanted to do with it — "open the issue,
// work on it, and talk to the workspace agents" — because a modal is a thing
// you must close before you can do anything else, and closing it is leaving the
// issue. A tab is a place you can stand.
//
// Everything inside it is the tracker's own: the same list and board
// (core/trackerIssuesPane.js), the same issue page with its timeline, composer
// and rail fields (core/trackerIssuePage.js). This file is the narrowing and
// the wiring, and nothing else — a second copy of any of that is how the two
// would drift apart.
//
// # Why the route stays a workspace route
//
// `#/…/workspace/<w>/issues[/<issueId>]` is still `name: "workspace"`, so the
// shell keys the rail on `workspace:<w>` exactly as it does for the directory
// tabs (core/shell.js). The bubbles do not move when the reader switches to
// this tab, opens an issue, or goes back — which is the whole point: the agent
// holding the issue is right there while it is being read.

import { routeProjectKey } from "./deviceKey.js";
import { mountIssuesPane } from "./trackerIssuesPane.js";
import { mountIssuePage } from "./trackerIssuePage.js";
import { workspaceAgents } from "./trackerAssignee.js";
import { assignedTo } from "./trackerAgentIssues.js";
import { hashFromRoute } from "./router.js";

/** The ids of the agents standing in one workspace, off the same groups the
 *  assignee picker is built from — so "an agent of this workspace" means the
 *  same thing in the filter and in the picker. */
export function agentIdsOfWorkspace(feed, projectKey, workspaceId) {
  const group = workspaceAgents(feed, projectKey).find((one) => one.workspaceId === workspaceId);
  return (group?.agents || []).map((agent) => agent.id).filter(Boolean);
}

/**
 * Whether one issue belongs on this workspace's tab.
 *
 * Assigned only, and assigned to an agent of this workspace. Not what those
 * agents merely track: a tracked issue is context for reading what an agent
 * says and belongs in its conversation, while this tab answers "what is the
 * work here". And not an issue held by the user or the project agent, which
 * are not standing in any workspace.
 */
export const heldHere = (issue, agentIds) => agentIds.some((agentId) => assignedTo(issue, agentId));

/** Where the tab itself stands, with no directory on it: a directory scopes
 *  Changes and Files and has nothing to say about which issues the agents
 *  here are holding, so it is left out of the URL entirely. */
export const workspaceIssuesPlace = (route) => ({
  name: "workspace",
  deviceId: route.deviceId,
  projectId: route.projectId,
  workspaceId: route.workspaceId,
  tab: "issues",
});

/** The route one issue opens on from inside this workspace. Still the
 *  workspace, so the rail beside it never moves. */
export const workspaceIssueRoute = (route, issueId) => ({ ...workspaceIssuesPlace(route), issueId });

/** The way out of this tab to the project's whole tracker (#117): the same
 *  arrow the chat overview's workspace scope wears to reach every workspace. */
export const projectIssuesRoute = (route) => ({
  name: "project",
  deviceId: route.deviceId,
  projectId: route.projectId,
  tab: "issues",
});

/**
 * Mount the tab: one issue's page when the route names one, the list or the
 * board when it does not.
 *
 * `feed` is read at mount and again on every feed move rather than captured,
 * because a workspace gains and loses agents while the tab stands there — an
 * agent added while the reader is looking at it should bring its issues with
 * it.
 *
 * `navigate` is handed in rather than imported: this module is about the
 * tracker inside a workspace, and a core module that reaches for the app is a
 * core module the tracker's own suites cannot mount.
 */
export function mountWorkspaceIssuesTab(body, { route, context, feed, selection, sayWhichIssue, navigate }) {
  const projectKey = routeProjectKey(route);
  const shared = {
    projectId: route.projectId,
    deviceId: context.deviceId,
    projectKey,
    callRpc: context.rpc,
    catalog: () => context.modelCatalog(),
    refreshCatalog: () => context.refreshModelCatalog(),
    feed,
    navigate,
    agentSelection: selection,
  };

  if (route.issueId) {
    body.innerHTML = `<div id="issue-pane" class="issue-surface"></div>`;
    return mountIssuePage(body.querySelector("#issue-pane"), {
      ...shared,
      issueId: route.issueId,
      onIssueRead: sayWhichIssue,
    });
  }

  // Named for what it is, not `.issue-pane`: that sat one character from the
  // `#issue-pane` the issue page above mounts into, and two hosts a selector
  // apart is how a test asserts the wrong one.
  body.innerHTML = `<div class="workspace-issues-pane"></div>`;
  return mountIssuesPane(body.querySelector(".workspace-issues-pane"), {
    ...shared,
    projectName: route.projectId,
    view: route.view,
    defaultView: "list",
    scopeLink: { label: "All project issues", href: hashFromRoute(projectIssuesRoute(route)) },
    only: (issue) => heldHere(issue, agentIdsOfWorkspace(feed(), projectKey, route.workspaceId)),
    issueRoute: (issue) => workspaceIssueRoute(route, issue.id),
    onViewChange: (view) => navigate?.({ ...workspaceIssuesPlace(route), view }),
  });
}
