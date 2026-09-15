// Turning a URL that does not say enough into a work item.
//
// Two kinds of URL end up here. The old ones addressed runs, worktrees, plans
// and primary checkouts by id; the new ones address a branch by (project,
// branch name) and an issue by (project, issue id), so the ids that survived
// need a lookup. And any work URL that names no device needs one too: every
// device mints a `proj-1`, so a bare project id names a project only once
// something says which machine it is on.
//
// The feed carries every half — its items[] rows, its projects and its
// workspaces all know the device that answered — so this is a lookup plus a
// policy, and both are pure: the caller supplies the rows and says which
// device is home.

/** The first candidate on a device the account lists, in the account's own
 *  order — the caller passes the ONLINE ids, so an offline machine's copy is
 *  never the one a link opens while another device has it. */
function firstListed(candidates, deviceOrder) {
  for (const deviceId of deviceOrder || []) {
    const found = candidates.find((candidate) => candidate.deviceId === deviceId);
    if (found) return found;
  }
  return null;
}

/** The candidate on the home device, when there is a home device to ask about. */
const atHome = (candidates, homeDeviceId) =>
  homeDeviceId ? candidates.find((candidate) => candidate.deviceId === homeDeviceId) : null;

/**
 * Which machine's copy the reader meant: the home device's, else the first
 * online device's in the account's order, else whatever came first. Null when
 * there is nothing to pick.
 */
export function pickDevice(candidates, policy = {}) {
  const rows = candidates || [];
  return atHome(rows, policy.homeDeviceId) || firstListed(rows, policy.deviceOrder) || rows[0] || null;
}

/** The surface a row opens, on the tab the URL named: a row that belongs to a
 *  workspace opens that workspace, otherwise the branch checkout. Null when the
 *  row names neither: an entity no URL can address belongs on the inbox. */
const branchRouteFor = (row, ref) => {
  if (row.workspace_id) {
    return {
      name: "workspace",
      projectId: row.project_id,
      workspaceId: row.workspace_id,
      ...(row.source_id ? { sourceId: row.source_id } : null),
      tab: ref.tab || "changes",
    };
  }
  return row.branch ? { name: "branch", projectId: row.project_id, branch: row.branch, tab: ref.tab || "changes" } : null;
};

/** An issue whose implementation is in flight has no row of its own — the
 *  branch row carries its id, and the branch is the nearest surface the URL can
 *  open. */
function issueRouteFor(row, ref) {
  if (row.kind === "branch") return branchRouteFor(row, ref);
  const route = { name: "issue", projectId: row.project_id, id: ref.id };
  if (ref.stage) route.stage = ref.stage;
  return route;
}

const byId = (field) => (ref, feed) => feed.items.filter((row) => row[field] === ref.id);

/**
 * What each kind of unresolved reference is looked up as: the rows a feed
 * offers as candidates for it, and the route the chosen one opens.
 *
 * A `project` ref is a work URL that named no device; the rest are legacy ids.
 */
const REFERENCE_KINDS = Object.freeze({
  run: { rows: byId("run_id"), route: branchRouteFor },
  worktree: { rows: byId("worktree_id"), route: branchRouteFor },
  // A primary checkout is named by its project alone, so the project has to
  // match — the primary row of some other project is not what the URL meant.
  primary: {
    rows: (ref, feed) => feed.items.filter((row) => row.primary && row.project_id === ref.projectId),
    route: branchRouteFor,
  },
  issue: { rows: byId("issue_id"), route: issueRouteFor },
  // A plain folder has no work row at all, so the projects answer for it, and a
  // workspace link that named no device is answered by the workspaces: each is
  // only asked which device it is on, and the URL already said the rest.
  project: {
    rows: (ref, feed) => [
      ...feed.items.filter((row) => row.project_id === ref.projectId),
      ...feed.projects.filter((project) => (project.project_id || project.id) === ref.projectId),
      ...feed.workspaces.filter((workspace) => workspace.project_id === ref.projectId),
    ],
    route: (row, ref) => ref.route || null,
  },
});

/** The rows that could be what the URL meant, most likely first: a row in the
 *  project the URL named beats one in any other project. */
const inNamedProjectFirst = (rows, projectId) =>
  projectId ? [...rows.filter((row) => row.project_id === projectId), ...rows.filter((row) => row.project_id !== projectId)] : rows;

/** The rows on the machine the URL named, when it named one: a URL carrying a
 *  device is not asking which machine it meant, so the other devices' copies of
 *  the same id are not candidates for it. */
const onNamedDevice = (rows, deviceId) => (deviceId ? rows.filter((row) => row.deviceId === deviceId) : rows);

/** The three collections a lookup reads, each defaulted: a feed carrying none of
 *  them is still a feed, with nothing in it to answer by. */
const lookupCollections = (feed) => ({ items: feed?.items || [], projects: feed?.projects || [], workspaces: feed?.workspaces || [] });

/** The device rides on the answer; a row that names none (a fixture, a feed
 *  from before rows were stamped) leaves the route as it found it. */
const onItsDevice = (route, row) => (route && row.deviceId ? { ...route, deviceId: row.deviceId } : route);

/**
 * The route a reference points at, or null when this feed carries nothing by
 * that id (deleted, or not yet polled — the caller decides whether to wait or
 * land on the inbox).
 *
 * @param ref {kind: 'run'|'worktree'|'issue'|'primary'|'project', id?, projectId?, deviceId?, route?, tab?, stage?}
 * @param feed {items, projects, workspaces} — the merge, every device's rows at once
 * @param policy {homeDeviceId, deviceOrder} — which device wins a collision
 */
export function resolveLegacyRoute(ref, feed, policy) {
  const kind = ref ? REFERENCE_KINDS[ref.kind] : null;
  if (!kind) return null;
  const rows = onNamedDevice(kind.rows(ref, lookupCollections(feed)), ref.deviceId);
  const chosen = pickDevice(inNamedProjectFirst(rows, ref.projectId), policy);
  return chosen ? onItsDevice(kind.route(chosen, ref), chosen) : null;
}

