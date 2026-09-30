// The project page's main pane, as a pure model: the workspaces one project
// holds, in the order they already stand in.
//
// The rail groups every device's workspaces into project blocks
// (core/inboxProjects.js). The page is one of those blocks opened on its own,
// so the grouping is reused rather than repeated: a row here and the same row
// on the rail carry the same route, the same unread and the same order.
//
// What a block does not carry is what a workspace is standing on — the page has
// room for the branch and the state, so those are read off the workspace record
// beside the row.
//
// No DOM, no app imports; views/projectView.js renders these.

import { workspaceEntries } from "./inbox.js";
import { workspaceProjectBlocks } from "./inboxProjects.js";
import { routeProjectKey } from "./deviceKey.js";
import { workspaceStatusText } from "./workspaceModel.js";
import { humanBytes, lifecycleView } from "./workspaceLifecycle.js";

/** The branch a workspace is standing on: the first of its sources that is on
 *  one. A workspace holds several checkouts and the row is one line, so the
 *  first is what there is room to say; "" when none of them is a repository. */
const branchOf = (workspace) =>
  (workspace?.directories || []).map((directory) => directory.branch).find(Boolean) || "";

/** What the bridge last measured the workspace at, or null before it has
 *  (#167): a sweep of a quiet workspace, or the Workspaces tab asking (#273). */
const sizeOf = (workspace) => {
  const bytes = workspace?.lifecycle?.size_bytes;
  return Number.isFinite(bytes) ? bytes : null;
};

/** The quiet stand-in for a size the machine is still to send (#273). */
export const SIZE_PLACEHOLDER = "—";

/** What a row says it weighs. A size the machine will send is a quiet
 *  placeholder until it arrives; a row whose size is not coming (an older
 *  machine, a workspace it never measures) says nothing, as it always has. */
const sizeText = (bytes, coming) => {
  if (bytes !== null) return humanBytes(bytes);
  return coming ? SIZE_PLACEHOLDER : "";
};

/** Whether the machine will send this workspace's size when asked: it
 *  measures only the workspaces Build made that are not finished. */
const sizeComing = (workspace, measuresSizes) =>
  measuresSizes && sizeOf(workspace) === null && workspace?.managed !== false && workspace?.status !== "finished";

/** One workspace as the page lists it: the rail's row, plus what that workspace
 *  is standing on, how its checkout is doing, whether it can be reclaimed, and
 *  what it weighs. */
const pageRow = (entry, workspace, measuresSizes) => ({
  key: entry.key,
  workspaceId: entry.workspaceId,
  workspaceKey: entry.workspaceKey,
  name: entry.name,
  route: entry.route,
  state: entry.state,
  unreadCount: entry.unreadCount,
  muted: entry.muted,
  facts: entry.facts,
  branch: branchOf(workspace),
  status: workspace?.status || "",
  statusText: workspaceStatusText(workspace),
  // The reclaim service's verdict (#135): null until it has one worth saying.
  lifecycle: lifecycleView(workspace?.lifecycle),
  sizeBytes: sizeOf(workspace),
  sizeText: sizeText(sizeOf(workspace), sizeComing(workspace, measuresSizes)),
  sizePending: sizeComing(workspace, measuresSizes),
});

/** The one project block the rail would paint for this project, or null when no
 *  device has listed it. Built from the rail's own grouping so the page and the
 *  rail cannot disagree about what is in a project. */
function projectBlock(feed, projectKey) {
  const workspaces = feed?.workspaces || [];
  const projects = feed?.projects || [];
  const entries = workspaceEntries(workspaces, projects, feed?.items || []);
  const { blocks, recentBlocks } = workspaceProjectBlocks(entries, projects);
  return [...blocks, ...recentBlocks].find((candidate) => candidate.projectKey === projectKey) || null;
}

/** The block's rows, each read beside the workspace record it came from. */
function pageRows(feed, block, measuresSizes) {
  const byKey = new Map((feed?.workspaces || []).map((workspace) => [workspace.workspaceKey, workspace]));
  return [...(block?.entries || []), ...(block?.recent || [])]
    .sort((left, right) => (left.anchorMs ?? Infinity) - (right.anchorMs ?? Infinity))
    .map((entry) => pageRow(entry, byKey.get(entry.workspaceKey), measuresSizes));
}

/**
 * The project page for the project a route is standing in.
 *
 * `feed` is the merge every surface reads (core/taskFeed.js); `route` says which
 * project, and a project is named by the pair (device, project id) — both
 * machines mint a `proj-1`, so a route with no machine on it names no project
 * and the page stands empty until the resolve hop supplies one.
 *
 * `measuresSizes` is the cached fact that the machine sends sizes when the
 * tab asks (core/workspaceSizeSupport.js).
 */
export function projectPageModel(feed, route, sizes) {
  const projectKey = routeProjectKey(route);
  const block = projectBlock(feed, projectKey);
  const rows = pageRows(feed, block, sizes?.measuresSizes === true);
  return {
    projectKey,
    projectId: route?.projectId || null,
    deviceId: route?.deviceId || null,
    name: block?.name || route?.projectId || "",
    rows,
    unreadCount: rows.reduce((total, row) => total + row.unreadCount, 0),
    empty: rows.length === 0,
  };
}

/** The Workspaces tab's two filters (#167): every workspace, or only those the
 *  reclaim service found nothing holding. */
export const ALL_WORKSPACES = "all";
export const RECLAIMABLE_WORKSPACES = "reclaimable";

const largestFirst = (left, right) => (right.sizeBytes ?? -1) - (left.sizeBytes ?? -1);

/**
 * The rows one filter shows. Every workspace keeps the rail's order, so the
 * page and the rail still read as one list; the reclaimable ones come largest
 * first, because that is the order a cleanup wants them in. An unknown filter
 * is every workspace.
 */
export function workspaceListing(page, filter) {
  const reclaimable = page.rows.filter((row) => row.lifecycle?.reclaimable === true);
  const narrowed = filter === RECLAIMABLE_WORKSPACES;
  const rows = narrowed ? [...reclaimable].sort(largestFirst) : page.rows;
  return {
    filter: narrowed ? RECLAIMABLE_WORKSPACES : ALL_WORKSPACES,
    rows,
    empty: rows.length === 0,
    reclaimableCount: reclaimable.length,
  };
}
