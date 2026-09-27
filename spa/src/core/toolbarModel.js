// Pure identities and menu rows for the view-area toolbar. Workspace routes
// show one picker reading `project / workspace`; its popup moves between the
// scoped project's workspaces and the project list, and reaches the project's
// own page. core/toolbar.js renders and wires it.

import { fuzzyRank } from "./fuzzy.js";
import { deviceTags, projectNameOf } from "./inboxProjects.js";
import { routeProjectKey, routeWorkspaceKey } from "./deviceKey.js";
import { workspaceDisplayName } from "./workspaceModel.js";

/** The branch the daemon will cut for a typed name, mirrored for the preview
 *  only — the daemon is still the one that decides. */
export function branchNamePreview(name) {
  const slug = String(name || "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 50)
    .replace(/-+$/g, "");
  return slug ? `build/${slug}` : "";
}

/** What one feed row is owed on the badge. The bridge sends the flag and the
 *  count off the same fact, so a row flagged with no count still weighs one. */
function unreadOf(item) {
  const count = Number(item && item.unread_count) || 0;
  if (count > 0) return count;
  return item && item.unread ? 1 : 0;
}

/** The project selector's menu: every machine's projects, the scoped one
 *  marked, filtered by name (subsequence matching, core/fuzzy.js). Projects
 *  only — a branch is not an answer to "which project".
 *
 *  A project is named by the pair (device, project): both machines mint a
 *  `proj-1`, so the scope, the marking and the counting are all by projectKey.
 *  A name two machines share says the device after it — the rail's own rule, on
 *  the rail's own set (core/inboxProjects.js) — and a name the account uses
 *  once says nothing, so a one-device account reads as it always has.
 *
 *  A project's counter is the unread of everything inside it — the work rows
 *  and the captures waiting to be routed there — because the project itself
 *  holds no conversation of its own to be unread in. */
export function projectMenuModel({ projects = [], items = [], devices = [], projectKey = null, query = "" } = {}) {
  const tags = deviceTags(projects, devices);
  const entries = projects.map((project) => ({
    key: project.projectKey,
    id: project.id,
    deviceId: project.deviceId,
    name: projectNameOf(project),
    ...tags.get(project.projectKey),
    current: project.projectKey === projectKey,
    unreadCount: items.reduce((total, item) => total + (item.projectKey === project.projectKey ? unreadOf(item) : 0), 0),
  }));
  return fuzzyRank(entries, query, (entry) => entry.name);
}

const WORKSPACE_STATUS_ORDER = new Map([
  ["ready", 0],
  ["provisioning", 1],
  ["failed", 2],
  ["finished", 4],
]);

function compareWorkspaceStatus(left, right) {
  const leftStatus = left.status || "";
  const rightStatus = right.status || "";
  return (WORKSPACE_STATUS_ORDER.get(leftStatus) ?? 3) - (WORKSPACE_STATUS_ORDER.get(rightStatus) ?? 3)
    || leftStatus.localeCompare(rightStatus);
}

/** Workspaces registered for one project, filtered by their human name and
 *  grouped by status, ready first and finished last. Within each status retain
 *  the listed order, or match relevance when filtering. A
 *  project belongs to one machine, so the narrowing and the marking are both by
 *  the account-wide names (core/deviceKey.js) the feed stamped on every row. */
export function workspaceMenuModel({ workspaces = [], projectKey = null, workspaceKey = null, query = "" } = {}) {
  const entries = workspaces
    .filter((workspace) => !projectKey || workspace.projectKey === projectKey)
    .map((workspace) => ({
      ...workspace,
      // What the user called it, not the slug its checkout folder took
      // (core/workspaceModel.js). A row with nothing to be called at all falls
      // back to its id, which is at least the thing the menu is addressing.
      name: workspaceDisplayName(workspace, workspace.id),
      current: workspace.workspaceKey === workspaceKey,
    }));
  return fuzzyRank(entries, query, (entry) => entry.name).sort(compareWorkspaceStatus);
}

/** What each kind of work route is: the row it stands on, what the bar calls it,
 *  and whatever else that kind carries. A route names one kind, so the bar reads
 *  its answer here rather than walking the kinds. */
const STANDING = {
  workspace: {
    // A workspace is not a feed row: it is a record of its own, found among the
    // workspaces below rather than among the items.
    rowIs: () => () => false,
    // The bar says what the user called this workspace, never the slug its
    // folder and branch were cut from (core/workspaceModel.js). Until that
    // machine has listed it there is no name to say, and the id the URL carries
    // is the only honest stand-in.
    label: (route, row, carried) =>
      (carried.workspace ? workspaceDisplayName(carried.workspace, route.workspaceId) : route.workspaceId) ?? "Workspace",
    // The picker is the bar's one control on a workspace: the directories, the
    // Tasks and the settings are the workspace's navigation, and stand in its
    // rail (core/directoryRail.js), not here.
    carries: (route, { workspaces }) => ({
      workspaceId: route.workspaceId,
      workspace: workspaces.find((candidate) => candidate.workspaceKey === routeWorkspaceKey(route)) || null,
    }),
  },
  branch: {
    // The machine and the project together name a branch row: both machines
    // mint a `proj-1` with a `main` in it, and those are two rows.
    rowIs: (route) => (item) =>
      item.kind === "branch" &&
      item.deviceId === route.deviceId &&
      item.project_id === route.projectId &&
      item.branch === route.branch,
    label: (route) => route.branch || "",
  },
  task: {
    // A task id is a uuid: it names one task wherever it is.
    rowIs: (route) => (item) => item.kind === "task" && item.task_id === route.id,
    label: (route, row) => (row && row.title) || "Task",
  },
  project: {
    // A project is the block the rows sit in rather than a row of its own, and
    // the page is about the project and nothing inside it: the bar says its
    // name through the project selector, then the project's two pages as tabs.
    rowIs: () => () => false,
    label: () => "",
    carries: (route, { tasksUnread }) => ({ projectTabs: projectTabsModel(route.tab, tasksUnread) }),
  },
  trackerTask: {
    // One task of the tracker is a page OF the project's Tasks tab: the bar
    // says the project, and the tabs stand with Tasks open, so the list is
    // one press away from the task — on a phone, the only press back.
    rowIs: () => () => false,
    label: () => "",
    carries: (route, { tasksUnread }) => ({ projectTabs: projectTabsModel("tasks", tasksUnread) }),
  },
};

/** The two pages a project has, as tabs after its name in the bar: its task
 *  tracker and the workspaces cut from it, marked with the one the route is on.
 *  They live in the bar rather than over the page so they stay reachable with
 *  the chat open over the page on a phone.
 *
 *  Tasks first, and the one a route that names no tab is on (#46): "I think
 *  tasks should be the first and primary project tab." Tasks wears the
 *  unread of every watched task in the project (#104). */
export function projectTabsModel(current, tasksUnread = 0) {
  const onWorkspaces = current === "workspaces";
  return [
    { id: "tasks", label: "Tasks", current: !onWorkspaces, unread: tasksUnread },
    { id: "workspaces", label: "Workspaces", current: onWorkspaces, unread: 0 },
  ];
}

/** What the bar says when the route is no work item at all. */
const NOWHERE = Object.freeze({ projectId: null, projectKey: null, project: "", kind: null, label: "", row: null });

/** Where the toolbar says you are standing: the project, and the workspace,
 *  branch or task inside it. The route is the authority on identity (it is what
 *  a deep link carries, machine included); the feed only supplies the names it
 *  knows, and the record it names is the one on the route's own machine. */
export function toolbarIdentity(route = {}, { items = [], projects = [], workspaces = [], tasksUnread = 0 } = {}) {
  const standing = STANDING[route.name];
  if (!standing) return NOWHERE;
  const key = routeProjectKey(route);
  const row = items.find(standing.rowIs(route)) || null;
  const carried = standing.carries ? standing.carries(route, { items, projects, workspaces, tasksUnread }) : null;
  return {
    projectId: route.projectId,
    projectKey: key,
    project: projectName(key, projects, row),
    kind: route.name,
    label: standing.label(route, row, carried),
    row,
    ...carried,
  };
}

function projectName(projectKey, projects, row) {
  const project = projects.find((entry) => entry.projectKey === projectKey);
  if (project) return projectNameOf(project);
  return (row && row.project) || "";
}
