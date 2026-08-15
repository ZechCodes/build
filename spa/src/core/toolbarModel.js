// The view-area toolbar's pure model: what the two selectors say, what each
// one's menu offers, and what the right side reports.
//
// The toolbar names where you are standing — project, then branch or issue —
// and each name opens the menu of its own kind: the project half lists projects,
// the item half lists the scoped project's branches and issues. Two questions,
// two lists; a menu that answered both at once made the same list appear behind
// both names. Everything here is pure; core/toolbar.js renders and wires it.

import { fuzzyRank } from "./fuzzy.js";

/** Bare duration, the vocabulary the inbox facts already speak: "<1m", "12m",
 *  "3h", "2d". */
export function humanDuration(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  if (s < 60) return "<1m";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

/** How long the turn in flight has been running, from the stamp when it parses
 *  (so a toolbar left on screen ticks) and from the bridge's own count when it
 *  does not. null when nothing is working. */
export function workingSeconds(workingTime, nowMs) {
  if (!workingTime) return null;
  const started = Date.parse(workingTime.since || "");
  if (Number.isFinite(started)) return (nowMs - started) / 1000;
  return Number.isFinite(workingTime.seconds) ? workingTime.seconds : null;
}

/** The diffstat as the toolbar says it: additions and deletions, nothing else.
 *  A row that predates the object shape sends the string ready-made. */
export function statText(stat) {
  if (!stat) return "";
  if (typeof stat === "string") return stat;
  const insertions = stat.insertions || 0;
  const deletions = stat.deletions || 0;
  if (!insertions && !deletions) return "";
  return `+${insertions} −${deletions}`;
}

/** The toolbar's right side: the working-time ticker and the diffstat, each ""
 *  when the row does not know it. */
export function toolbarStatus(row, nowMs = Date.now()) {
  const working = workingSeconds(row && row.working_time, nowMs);
  return {
    working: working === null ? "" : `working ${humanDuration(working)}`,
    stat: statText(row && row.stat),
  };
}

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

/** One feed row as the menu lists it. `key` names it in the DOM; `route` is
 *  where picking it goes. */
function toEntry(item) {
  const isIssue = item.kind === "issue";
  return {
    key: isIssue ? `issue:${item.issue_id}` : `branch:${item.project_id}:${item.branch}`,
    kind: item.kind,
    label: isIssue ? item.title || "(untitled issue)" : item.branch || "(detached)",
    detail: isIssue ? "Issue" : item.title || "",
    unread: !!item.unread,
    working: !!item.working,
    route: isIssue
      ? { name: "issue", projectId: item.project_id, id: item.issue_id }
      : { name: "branch", projectId: item.project_id, branch: item.branch, tab: "changes" },
    resumeMs: ms(item.resume_at),
  };
}

/** The project selector's menu: the projects the device knows, the scoped one
 *  marked, filtered by name (subsequence matching, core/fuzzy.js). Projects
 *  only — a branch is not an answer to "which project". */
export function projectMenuModel({ projects = [], projectId = null, query = "" } = {}) {
  const entries = projects.map((project) => ({
    id: project.id,
    name: project.name || project.id,
    current: project.id === projectId,
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
