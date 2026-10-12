// A complete directory of cached destinations, shared by navigation surfaces.
// The rail selects watched activity; the picker and management inventory read
// every live named child. Ordering and filtering never depend on unread or on
// connection state, and building/searching the catalog is synchronous.
import { stampProject, stampWorkspace } from "./feedMerge.js";
import { byProjectName, deviceTags, projectNameOf } from "./inboxProjects.js";
import { projectRoute, workspaceRoute } from "./projectModel.js";
import { standsOnProjectCheckout, workspaceDisplayName, workspaceStatusText } from "./workspaceModel.js";
import { fuzzyScore } from "./fuzzy.js";
import { destinationTarget } from "./navigationRoutes.js";
import { navigationUnreadCounts } from "./navigationUnread.js";

const UNKNOWN_LIST = Object.freeze({ present: false, observedAt: null });
const byWorkspaceName = (left, right) => left.name.localeCompare(right.name, undefined, { sensitivity: "base" })
  || left.key.localeCompare(right.key);

/** A workspace's parent may not yet have a listed name. Its device and wire
 * project ID still identify a destination; never borrow another scope's name. */
function catalogProjects(projects, workspaces) {
  const named = new Map(projects.map((project) => [project.projectKey, project]));
  for (const workspace of workspaces) {
    if (named.has(workspace.projectKey)) continue;
    const parent = stampProject({ id: workspace.project_id, name: workspace.project || workspace.project_id }, workspace.deviceId);
    named.set(parent.projectKey, parent);
  }
  return [...named.values()];
}

function workspaceDestination(workspace, unread, context) {
  const destination = {
    kind: "workspace", key: workspace.workspaceKey, workspaceKey: workspace.workspaceKey,
    projectKey: workspace.projectKey, deviceId: workspace.deviceId, projectId: workspace.project_id,
    workspaceId: workspace.id, name: workspaceDisplayName(workspace, workspace.id),
    status: workspace.status, statusText: workspaceStatusText(workspace),
    unreadCount: unread.workspaces.get(workspace.workspaceKey) ?? 0,
    workspace, route: workspaceRoute(workspace),
  };
  return { ...destination, ...destinationTarget(destination, context) };
}

function projectGroup(project, workspaces, tag, unread, feed, context) {
  const destination = {
    kind: "project", key: project.projectKey, projectKey: project.projectKey,
    deviceId: project.deviceId, projectId: project.id, name: projectNameOf(project),
    unreadCount: unread.projectOwn.get(project.projectKey) ?? 0,
    project, route: projectRoute(project),
  };
  const view = feed.devices ? feed.devices[project.deviceId] : feed;
  return {
    key: project.projectKey, projectKey: project.projectKey, deviceId: project.deviceId,
    name: destination.name, ...tag, deviceName: tag.deviceName || project.deviceId,
    unreadCount: unread.projects.get(project.projectKey) ?? 0,
    roster: view?.listKnowledge?.workspaces || UNKNOWN_LIST,
    destination: { ...destination, ...destinationTarget(destination, context) },
    workspaces: workspaces.filter((workspace) => workspace.projectKey === project.projectKey)
      .map((workspace) => workspaceDestination(workspace, unread, context)).sort(byWorkspaceName),
  };
}

/** Feed listKnowledge comes from canonical cache records, per device. A
 * default [] in a feed fixture cannot establish that the roster was observed.
 * Pass the watched-task follower's taskUnread tally to share the rail counts. */
export function navigationCatalog(feed = {}, { devices = [], route = {}, currentHash = null, taskUnread } = {}) {
  const projects = (feed.projects || []).map((project) => stampProject(project, project.deviceId));
  const workspaces = (feed.workspaces || []).map((workspace) => stampWorkspace(workspace, workspace.deviceId));
  const named = catalogProjects(projects, workspaces);
  const projectsByKey = new Map(named.map((project) => [project.projectKey, project]));
  const live = workspaces.filter((workspace) => workspace.status !== "finished"
    && !standsOnProjectCheckout(workspace, projectsByKey.get(workspace.projectKey)));
  const unread = navigationUnreadCounts({ ...feed, projects: named, workspaces, taskUnread });
  const tags = deviceTags(named, devices);
  const context = { route, currentHash };
  const groups = named.map((project) => projectGroup(project, live, tags.get(project.projectKey), unread, feed, context)).sort(byProjectName);
  return { projects: groups, destinations: groups.flatMap((group) => [group.destination, ...group.workspaces]) };
}

/** Management gets named children only, with cached presence kept separate
 * from emptiness. Its own filters/actions do not belong to the picker. */
export function projectWorkspaceInventory(catalog, projectKey) {
  const group = catalog.projects.find((project) => project.key === projectKey);
  return { ...(group?.roster || UNKNOWN_LIST), workspaces: group?.workspaces || [] };
}

const bestScore = (texts, query) => {
  const scores = texts.map((text) => fuzzyScore(text, query)).filter((score) => score !== null);
  return scores.length ? Math.max(...scores) : null;
};

function searchedGroup(group, query, local) {
  const contextScore = bestScore([group.name, group.deviceName, `${group.name} ${group.deviceName}`], query);
  const project = local ? { ...group.destination, returnToProject: true } : group.destination;
  const children = group.workspaces.map((row) => ({ row, score: bestScore([
    row.name, `${group.name} ${row.name} ${group.deviceName}`, `${row.name} ${group.name} ${group.deviceName}`,
  ], query) })).filter(({ score }) => contextScore !== null || score !== null);
  children.sort((left, right) => (right.score ?? contextScore) - (left.score ?? contextScore)
    || byWorkspaceName(left.row, right.row));
  const destinations = [...(contextScore !== null ? [project] : []), ...children.map(({ row }) => row)];
  const scores = [contextScore, ...children.map(({ score }) => score)].filter((score) => score !== null);
  return { ...group, destinations, score: scores.length ? Math.max(...scores) : null };
}

function unfilteredGroup(group, local) {
  const project = local ? { ...group.destination, returnToProject: true } : group.destination;
  const children = local ? [...group.workspaces].sort((left, right) => Number(right.current) - Number(left.current)
    || byWorkspaceName(left, right)) : group.workspaces;
  return { ...group, destinations: [project, ...children] };
}

/** Global starts across all devices; an optional picker device filter never
 * changes Current. Local starts with the project return then the current
 * child. Search includes context, retains headings and ranks before stable
 * name/key ties. No DOM, focus, persistence or RPC is involved. */
export function navigationPickerModel(catalog, { projectKey = null, deviceId = null, query = "" } = {}) {
  const local = projectKey !== null;
  const searching = !!String(query).trim();
  const scoped = catalog.projects.filter((group) => (!local || group.key === projectKey) && (!deviceId || group.deviceId === deviceId));
  let groups = scoped.map((group) => searching ? searchedGroup(group, query, local) : unfilteredGroup(group, local));
  if (searching) groups = groups.filter((group) => group.destinations.length).sort((left, right) => right.score - left.score || byProjectName(left, right));
  return { groups, roster: local ? projectWorkspaceInventory(catalog, projectKey) : null };
}
