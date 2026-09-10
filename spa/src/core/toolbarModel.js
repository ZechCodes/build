// The view-area toolbar's pure model: what the two selectors say, what each
// one's menu offers, and what the right side reports.
//
// The toolbar names where you are standing — project, then branch or issue —
// and each name opens the menu of its own kind: the project half lists projects,
// the item half lists the scoped project's branches and issues. Two questions,
// two lists; a menu that answered both at once made the same list appear behind
// both names. Everything here is pure; core/toolbar.js renders and wires it.

import { fuzzyRank } from "./fuzzy.js";

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

/** One feed row as the menu lists it. `key` names it in the DOM; `route` is
 *  where picking it goes. */
function toEntry(item) {
  const isIssue = item.kind === "issue";
  return {
    key: isIssue ? `issue:${item.issue_id}` : `branch:${item.project_id}:${item.branch}`,
    kind: item.kind,
    label: isIssue ? item.title || "(untitled issue)" : item.branch || "(detached)",
    detail: isIssue ? "Issue" : item.title || "",
    unreadCount: unreadOf(item),
    working: !!item.working,
    route: isIssue
      ? { name: "issue", projectId: item.project_id, id: item.issue_id }
      : { name: "branch", projectId: item.project_id, branch: item.branch, tab: "changes" },
    resumeMs: ms(item.resume_at),
  };
}

/** The project selector's menu: the projects the device knows, the scoped one
 *  marked, filtered by name (subsequence matching, core/fuzzy.js). Projects
 *  only — a branch is not an answer to "which project".
 *
 *  A project's counter is the unread of everything inside it — the work rows
 *  and the captures waiting to be routed there — because the project itself
 *  holds no conversation of its own to be unread in. */
export function projectMenuModel({ projects = [], items = [], projectId = null, query = "" } = {}) {
  const entries = projects.map((project) => ({
    id: project.id,
    name: project.name || project.id,
    current: project.id === projectId,
    unreadCount: items.reduce((total, item) => total + (item.project_id === project.id ? unreadOf(item) : 0), 0),
  }));
  return fuzzyRank(entries, query, (entry) => entry.name);
}

/** The item selector's menu: the branches and issues of the scoped project,
 *  most recently touched first, filtered by what they are called.
 *
 *  A row with no branch to name it by is nameable by no URL, so it is not on a
 *  menu whose whole job is navigation. */
export function workMenuModel({ items = [], projectId = null, query = "" } = {}) {
  const work = items
    .filter((item) => item.project_id === projectId)
    .filter((item) => (item.kind === "issue" ? !!item.issue_id : !!item.branch))
    .map(toEntry)
    .sort((a, b) => b.resumeMs - a.resumeMs || a.label.localeCompare(b.label));
  return fuzzyRank(work, query, (entry) => `${entry.label} ${entry.detail}`);
}

/** Where the toolbar says you are standing: the project, and the branch or
 *  issue inside it. The route is the authority on identity (it is what a deep
 *  link carries); the feed only supplies the names it knows. */
export function toolbarIdentity(route = {}, { items = [], projects = [] } = {}) {
  const rowOf = (predicate) => items.find(predicate) || null;
  if (route.name === "branch") {
    const row = rowOf((item) => item.kind === "branch" && item.project_id === route.projectId && item.branch === route.branch);
    return {
      projectId: route.projectId,
      project: projectName(route.projectId, projects, row),
      kind: "branch",
      label: route.branch || "",
      row,
    };
  }
  if (route.name === "issue") {
    const row = rowOf((item) => item.kind === "issue" && item.issue_id === route.id);
    return {
      projectId: route.projectId,
      project: projectName(route.projectId, projects, row),
      kind: "issue",
      label: (row && row.title) || "Issue",
      row,
    };
  }
  return { projectId: null, project: "", kind: null, label: "", row: null };
}

function projectName(projectId, projects, row) {
  const project = projects.find((entry) => entry.id === projectId);
  if (project) return project.name || project.id;
  return (row && row.project) || "";
}
