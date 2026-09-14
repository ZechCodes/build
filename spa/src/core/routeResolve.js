// Turning a URL that does not say enough into a work item.
//
// Two kinds of URL end up here. The old ones addressed runs, worktrees, plans
// and primary checkouts by id; the new ones address a branch by (project,
// branch name) and an issue by (project, issue id), so the ids that survived
// need a lookup. And any work URL that names no device needs one too: every
// device mints a `proj-1`, so a bare project id names a project only once
// something says which machine it is on.
//
// The feed carries both halves — its items[] rows and its projects both know
// the device that answered — so this is a lookup plus a policy, and both are
// pure: the caller supplies the rows and says which device is home.

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

/**
 * Which machine's copy the reader meant: the home device's, else the first
 * online device's in the account's order, else whatever came first. Null when
 * there is nothing to pick.
 */
export function pickDevice(candidates, { homeDeviceId, deviceOrder } = {}) {
  const rows = candidates || [];
  const atHome = homeDeviceId ? rows.find((row) => row.deviceId === homeDeviceId) : null;
  return atHome || firstListed(rows, deviceOrder) || rows[0] || null;
}

const branchRouteFor = (row, tab) =>
  row && row.branch ? { name: "branch", projectId: row.project_id, branch: row.branch, tab: tab || "changes" } : null;

/** An issue whose implementation is in flight has no row of its own — the
 *  branch row carries its id, and the branch is the nearest surface the URL can
 *  open. */
function issueRouteFor(row, ref) {
  if (row.kind === "branch") return branchRouteFor(row, ref.tab);
  const route = { name: "issue", projectId: row.project_id, id: ref.id };
  if (ref.stage) route.stage = ref.stage;
  return route;
}

const byId = (field) => (ref, feed) => feed.items.filter((row) => row[field] === ref.id);

/**
 * What each kind of unresolved reference is looked up as: the candidates a feed
 * offers for it, and the route the chosen one opens.
 *
 * A `project` ref is a work URL that named no device; the rest are legacy ids.
 * Adding the fifth case to an if-chain is what took this past the complexity
 * cap, and each kind reads as its own two lines here.
 */
const CANDIDATES = Object.freeze({
  run: { rows: byId("run_id"), route: (row, ref) => branchRouteFor(row, ref.tab) },
  worktree: { rows: byId("worktree_id"), route: (row, ref) => branchRouteFor(row, ref.tab) },
  // A primary checkout is named by its project alone, so the project has to
  // match — the primary row of some other project is not what the URL meant.
  primary: {
    rows: (ref, feed) => feed.items.filter((row) => row.primary && row.project_id === ref.projectId),
    route: (row, ref) => branchRouteFor(row, ref.tab),
  },
  issue: { rows: byId("issue_id"), route: issueRouteFor },
  // A plain folder has no work row at all, so the projects answer for it.
  project: {
    rows: (ref, feed) => [
      ...feed.items.filter((row) => row.project_id === ref.projectId),
      ...feed.projects.filter((project) => (project.project_id || project.id) === ref.projectId),
    ],
    route: (row, ref) => ref.route || null,
  },
});

/** The rows that could be what the URL meant, most likely first: a row in the
 *  project the URL named beats one in any other project. */
const inNamedProjectFirst = (rows, projectId) =>
  projectId ? [...rows.filter((row) => row.project_id === projectId), ...rows.filter((row) => row.project_id !== projectId)] : rows;

/**
 * The route a reference points at, or null when this feed carries nothing by
 * that id (deleted, or not yet polled — the caller decides whether to wait or
 * land on the inbox).
 *
 * @param ref {kind: 'run'|'worktree'|'issue'|'primary'|'project', id?, projectId?, route?, tab?, stage?}
 * @param feed {items, projects} — the merge, every device's rows at once
 * @param policy {homeDeviceId, deviceOrder} — which device wins a collision
 */
export function resolveLegacyRoute(ref, feed, policy) {
  const rule = ref ? CANDIDATES[ref.kind] : null;
  if (!rule) return null;
  const rows = rule.rows(ref, { items: feed?.items || [], projects: feed?.projects || [] });
  const chosen = pickDevice(inNamedProjectFirst(rows, ref.projectId), policy);
  const route = chosen ? rule.route(chosen, ref) : null;
  // The device rides on the answer; a row that names none (a fixture, a feed
  // from before rows were stamped) leaves the route as it found it.
  return route && chosen.deviceId ? { ...route, deviceId: chosen.deviceId } : route;
}
