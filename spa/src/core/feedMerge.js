// The feed's pure half: reading one device's wire, and making one inbox out of
// every device's reading of it.
//
// A row means nothing without the device it came from — two machines both call
// their first project `proj-1` — so the device is stamped on every row and
// project HERE, in the one place the wire is read, and the account-wide project
// name (core/deviceKey.js) is minted beside it. Wire fields are left exactly as
// they arrived: the bridge still wants the bare `project_id` back.
//
// The merge and a single device's view are the same shape, so a consumer that
// wants one device (the toolbar, the capture decision page) is written once
// against `deviceView(feed, id)` and a consumer that wants them all reads the
// merge itself.

import { deviceKey, workspaceKey } from "./deviceKey.js";

/** Where each collection comes from on the wire. The redesigned feed is one row
 *  per work item; the legacy collections below still ship, and still feed what
 *  has not moved over yet — including the lifecycle verbs whose git is running
 *  right now (`pending`), which a bridge that predates them never sends. */
const WIRE_FIELDS = Object.freeze({
  items: "items",
  plans: "plans",
  runs: "runs",
  externalWorktrees: "external_worktrees",
  pending: "pending",
  primaryChanges: "primary_changes",
});

/** The collections a snapshot carries: the board's, in the order the wire names
 *  them, and the projects and workspaces that come from the other two reads.
 *  Every one of them is an array of rows stamped with the device that answered,
 *  which is what lets a reader narrow a snapshot to one machine
 *  (core/deviceFilter.js) without knowing what any row is. */
export const FEED_COLLECTIONS = Object.freeze([...Object.keys(WIRE_FIELDS), "projects", "workspaces"]);

const EMPTY_VIEW = Object.freeze({
  ...Object.fromEntries(FEED_COLLECTIONS.map((field) => [field, Object.freeze([])])),
  cached: true,
});

/** A row as the account sees it: whose device answered, and which project on
 *  that device it belongs to. A row that names no project gets no key — there
 *  is nothing to name. */
const stampRow = (row, deviceId) =>
  row.project_id ? { ...row, deviceId, projectKey: deviceKey(deviceId, row.project_id) } : { ...row, deviceId };

/** The wire names a project by `project_id`; consumers of the snapshot (the
 *  toolbar's scope and menu) read `id`. Bridge the key here, and stamp the
 *  account-wide name beside it. */
function stampProject(project, deviceId) {
  const id = project.project_id || project.id;
  return { ...project, id, deviceId, projectKey: deviceKey(deviceId, id) };
}

/** A workspace as the account sees it: the machine it is checked out on, the
 *  project it belongs to there, its own account-wide name, and the board's
 *  summary of what is running in it when the board carries one. Exported
 *  because the toolbar reads `workspace.list` off one device for its switcher,
 *  and a workspace is named the same way wherever it was read. */
export function stampWorkspace(workspace, deviceId, summaries = []) {
  const id = workspace.workspace_id || workspace.id;
  const summary = summaries.find((candidate) => candidate.workspace_id === id);
  return {
    ...workspace,
    ...(summary ? { work_summary: summary.work_summary } : null),
    id,
    deviceId,
    projectKey: deviceKey(deviceId, workspace.project_id),
    workspaceKey: workspaceKey(deviceId, id),
  };
}

/** One device's snapshot, read from its `board.list`, `project.list` and
 *  `workspace.list`. A bridge that does not serve workspaces answers none. */
export function liveFeedSnapshot(board, projectList, workspaceList, deviceId) {
  const view = {};
  for (const [field, wire] of Object.entries(WIRE_FIELDS)) {
    view[field] = (board[wire] || []).map((row) => stampRow(row, deviceId));
  }
  view.projects = (projectList.projects || []).map((project) => stampProject(project, deviceId));
  const summaries = board.workspace_summaries || [];
  view.workspaces = (workspaceList?.workspaces || []).map((workspace) => stampWorkspace(workspace, deviceId, summaries));
  return view;
}

/** The devices' views in the order the account lists them; a device the list
 *  has not caught up with yet keeps its own place, last and stable. */
function orderedViews(byDevice, deviceOrder) {
  const rank = new Map(deviceOrder.map((deviceId, index) => [deviceId, index]));
  return [...byDevice.entries()].sort(
    ([first], [second]) => (rank.get(first) ?? rank.size) - (rank.get(second) ?? rank.size),
  );
}

/**
 * Every device's snapshot as one. The collections are concatenated in
 * device order; `devices` holds each device's own view, the same shape as the
 * merge, for the surfaces that are about one machine.
 *
 * `cached` says the whole merge is the cache talking — true only while every
 * device in it is a boot paint, so the sync layer never mistakes a live answer
 * for its own echo. A merge of no devices is nothing the syncer should act on,
 * so it is cached too.
 */
export function mergeFeeds(byDevice, deviceOrder = []) {
  const ordered = orderedViews(byDevice, deviceOrder);
  const merged = { devices: Object.fromEntries(ordered) };
  for (const field of FEED_COLLECTIONS) {
    merged[field] = ordered.flatMap(([, view]) => view[field] || []);
  }
  merged.cached = ordered.every(([, view]) => Boolean(view.cached));
  return merged;
}

/** One device's view of a snapshot. A snapshot that names no devices is already
 *  one device's view (a test's fixture, a single-device mock), so it is its own
 *  answer. */
export function deviceView(feed, deviceId) {
  if (!feed) return EMPTY_VIEW;
  if (!feed.devices) return feed;
  return feed.devices[deviceId] || EMPTY_VIEW;
}
