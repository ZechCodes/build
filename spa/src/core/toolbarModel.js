// The view-area toolbar's pure model: what the two selectors say, what each
// one's menu offers, and what the right side reports.
//
// The toolbar names where you are standing — project, then branch or issue —
// and each name opens the menu of its own kind: the project half lists projects,
// the item half lists the scoped project's branches and issues. Two questions,
// two lists; a menu that answered both at once made the same list appear behind
// both names. Everything here is pure; core/toolbar.js renders and wires it.

import { fuzzyRank } from "./fuzzy.js";
import { clashingProjectNames } from "./inboxProjects.js";
import { routeProjectKey } from "./deviceKey.js";

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

const ms = (iso) => {
  const parsed = Date.parse(iso || "");
  return Number.isFinite(parsed) ? parsed : 0;
};

/** What one feed row is owed on the badge. The bridge sends the flag and the
 *  count off the same fact, so a row flagged with no count still weighs one. */
function unreadOf(item) {
  const count = Number(item && item.unread_count) || 0;
  if (count > 0) return count;
  return item && item.unread ? 1 : 0;
}

/** One feed row as the menu lists it. `key` names it in the DOM — by the
 *  account-wide project name, since both machines mint a `proj-1` with a `main`
 *  in it; `route` is where picking it goes, on the machine it is on. */
function toEntry(item) {
  const isIssue = item.kind === "issue";
  return {
    key: isIssue ? `issue:${item.issue_id}` : `branch:${item.projectKey}:${item.branch}`,
    kind: item.kind,
    label: isIssue ? item.title || "(untitled issue)" : item.branch || "(detached)",
    detail: isIssue ? "Issue" : item.title || "",
    unreadCount: unreadOf(item),
    working: !!item.working,
    route: isIssue
      ? { name: "issue", deviceId: item.deviceId, projectId: item.project_id, id: item.issue_id }
      : { name: "branch", deviceId: item.deviceId, projectId: item.project_id, branch: item.branch, tab: "changes" },
    resumeMs: ms(item.resume_at),
  };
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
  const clashes = clashingProjectNames(projects);
  const deviceNames = new Map(devices.map((device) => [device.id, device.name]));
  const entries = projects.map((project) => {
    const name = project.name || project.id;
    return {
      key: project.projectKey,
      id: project.id,
      deviceId: project.deviceId,
      name,
      deviceName: deviceNames.get(project.deviceId) || null,
      clash: clashes.has(name),
      current: project.projectKey === projectKey,
      unreadCount: items.reduce((total, item) => total + (item.projectKey === project.projectKey ? unreadOf(item) : 0), 0),
    };
  });
  return fuzzyRank(entries, query, (entry) => entry.name);
}

/** The item selector's menu: the branches and issues of the scoped project —
 *  the one on the machine the scope names, never another machine's project of
 *  the same bare id —
 *  most recently touched first, filtered by what they are called.
 *
 *  A row with no branch to name it by is nameable by no URL, so it is not on a
 *  menu whose whole job is navigation. */
export function workMenuModel({ items = [], projectKey = null, query = "" } = {}) {
  const work = items
    .filter((item) => item.projectKey === projectKey)
    .filter((item) => (item.kind === "issue" ? !!item.issue_id : !!item.branch))
    .map(toEntry)
    .sort((a, b) => b.resumeMs - a.resumeMs || a.label.localeCompare(b.label));
  return fuzzyRank(work, query, (entry) => `${entry.label} ${entry.detail}`);
}

/** What each kind of work route is: the row it stands on, and what the bar
 *  calls it. A route names one kind, so the bar reads its answer here rather
 *  than walking the kinds. */
const STANDING = {
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
  issue: {
    // An issue id is a uuid: it names one issue wherever it is.
    rowIs: (route) => (item) => item.kind === "issue" && item.issue_id === route.id,
    label: (route, row) => (row && row.title) || "Issue",
  },
};

/** What the bar says when the route is no work item at all. */
const NOWHERE = Object.freeze({ projectId: null, projectKey: null, project: "", kind: null, label: "", row: null });

/** Where the toolbar says you are standing: the project, and the branch or
 *  issue inside it. The route is the authority on identity (it is what a deep
 *  link carries, machine included); the feed only supplies the names it knows,
 *  and the row it names is the one on the route's own machine. */
export function toolbarIdentity(route = {}, { items = [], projects = [] } = {}) {
  const standing = STANDING[route.name];
  if (!standing) return NOWHERE;
  const key = routeProjectKey(route);
  const row = items.find(standing.rowIs(route)) || null;
  return {
    projectId: route.projectId,
    projectKey: key,
    project: projectName(key, projects, row),
    kind: route.name,
    label: standing.label(route, row),
    row,
  };
}

function projectName(projectKey, projects, row) {
  const project = projects.find((entry) => entry.projectKey === projectKey);
  if (project) return project.name || project.id;
  return (row && row.project) || "";
}
