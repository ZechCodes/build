// Hash routes for the three-panel shell. There are two work items and two
// global surfaces, and that is the whole vocabulary:
//   #/inbox                                     — the landing surface
//   #/project/<projectId>/branch/<name>/<tab>   — tab is changes | files
//   #/project/<projectId>/issue/<issueId>[/stage/<stageId>]
//   #/account[/<page>]                          — page is settings | devices | archive
//
// Every URL the pre-redesign client could mint still opens the nearest of
// those. Conversation and Agent are the agent rail now, terminals are the
// console, and Diff merged into Changes, so those tabs all fold into Changes;
// the old right-cluster tabs (Inbox, Issues, Archive) named project-wide panes
// and go to the global surface that owns them.
//
// Some legacy URLs name an entity by an id whose branch this module cannot
// know (a run id, a worktree id, an issue with no project in the URL, a
// project's primary checkout). Those parse to a `resolve` route: the app looks
// the id up in the feed (core/routeResolve.js) and rewrites the hash.
//
// Pure mapping both ways; the app shell owns the hashchange listener.

const BRANCH_TABS = new Set(["changes", "files"]);
const ACCOUNT_PAGES = new Set(["settings", "devices", "archive"]);

const isTermTab = (segment) => /^term-\d+$/.test(segment || "");
// Tab segments a pre-redesign URL could carry. They are not destinations any
// more, but they still have to be RECOGNIZED as tabs — otherwise a trailing
// `conversation` would read as part of a slashed branch name.
const RETIRED_TABS = new Set(["conversation", "agent", "stages", "diff", "plan", "review", "inbox", "issues", "archive"]);
const isTabSegment = (segment) => BRANCH_TABS.has(segment) || RETIRED_TABS.has(segment) || isTermTab(segment);

// The retired tabs that named a project-wide pane rather than the entity's own
// work surface: whichever entity carried them, they belong to a global route.
const clusterRoute = (segment) =>
  segment === "inbox" || segment === "issues"
    ? { name: "inbox" }
    : segment === "archive"
      ? { name: "account", page: "archive" }
      : null;

// Files is the one entity tab that kept its name; everything else lands on
// Changes, including nothing at all.
const branchTab = (segment) => (BRANCH_TABS.has(segment) ? segment : "changes");

const inbox = () => ({ name: "inbox" });

/** A legacy stage deep-link: `<tab>/<stageId>` where the tab is one of the
 *  retired plan tabs (stages, review, conversation…). Returns the stage id, or
 *  undefined when the segments name no stage. */
function legacyStage(tabSegment, stageSegment) {
  if (!stageSegment) return undefined;
  return tabSegment === "stage" || isTabSegment(tabSegment) ? stageSegment : undefined;
}

/** `#/project/<p>/issue/<id>[…]` — the project is in the URL, so this is the
 *  canonical issue route no matter which legacy tail follows the id. */
function issueRoute(projectId, id, tailSegments) {
  const cluster = clusterRoute(tailSegments[0]);
  if (cluster) return cluster;
  const route = { name: "issue", projectId, id };
  const stage = legacyStage(tailSegments[0], tailSegments[1]);
  if (stage) route.stage = stage;
  return route;
}

/** The tail of a branch URL: every segment after `branch/`. A slashed branch
 *  name survives both encoded (one segment) and hand-typed (several), and a
 *  lone segment is always the branch — a branch may be named `changes`. */
function branchRoute(projectId, tailSegments) {
  if (!tailSegments.length) return inbox();
  const last = tailSegments[tailSegments.length - 1];
  const trailingTab = tailSegments.length > 1 && isTabSegment(last);
  if (trailingTab) {
    const cluster = clusterRoute(last);
    if (cluster) return cluster;
  }
  return {
    name: "branch",
    projectId,
    branch: (trailingTab ? tailSegments.slice(0, -1) : tailSegments).join("/"),
    tab: trailingTab ? branchTab(last) : "changes",
  };
}

/** A legacy entity URL whose branch this module cannot know. `kind` says what
 *  the id is; the app resolves it against the feed. */
function resolveRoute(kind, { projectId, id, tabSegment }) {
  const cluster = clusterRoute(tabSegment);
  if (cluster) return cluster;
  const route = { name: "resolve", kind };
  if (projectId) route.projectId = projectId;
  if (id) route.id = id;
  route.tab = branchTab(tabSegment);
  return route;
}

/** A legacy issue URL with no project in it: same parking spot, but issues have
 *  stages instead of tabs. */
function resolveIssueRoute(id, tailSegments) {
  const cluster = clusterRoute(tailSegments[0]);
  if (cluster) return cluster;
  const route = { name: "resolve", kind: "issue", id };
  const stage = legacyStage(tailSegments[0], tailSegments[1]);
  if (stage) route.stage = stage;
  return route;
}

export function routeFromHash(hash) {
  const parts = (hash || "")
    .replace(/^#\/?/, "")
    .split("/")
    .filter(Boolean)
    .map(decodeURIComponent);
  switch (parts[0]) {
    case "inbox":
      return inbox();
    case "account":
      return { name: "account", page: ACCOUNT_PAGES.has(parts[1]) ? parts[1] : "settings" };
    case "settings":
      return { name: "account", page: "settings" };
    case "task":
      if (!parts[1]) return inbox();
      return resolveRoute("run", { id: parts[1], tabSegment: parts[2] });
    case "issue":
    case "plan":
      if (!parts[1]) return inbox();
      return resolveIssueRoute(parts[1], parts.slice(2));
    case "worktree":
      if (!parts[1] || !parts[2]) return inbox();
      return resolveRoute("worktree", { projectId: parts[1], id: parts[2], tabSegment: parts[3] });
    case "main":
      if (!parts[1]) return inbox();
      return resolveRoute("primary", { projectId: parts[1], tabSegment: parts[2] });
    case "project": {
      if (!parts[1]) return inbox();
      const projectId = parts[1];
      if (parts[2] === "branch") return branchRoute(projectId, parts.slice(3));
      if (parts[2] === "issue" || parts[2] === "plan") {
        if (!parts[3]) return inbox();
        return issueRoute(projectId, parts[3], parts.slice(4));
      }
      if (parts[2] === "task") {
        if (!parts[3]) return inbox();
        return resolveRoute("run", { projectId, id: parts[3], tabSegment: parts[4] });
      }
      if (parts[2] === "worktree") {
        if (!parts[3]) return inbox();
        return resolveRoute("worktree", { projectId, id: parts[3], tabSegment: parts[4] });
      }
      // Everything else under a project was the primary checkout's surface.
      return resolveRoute("primary", { projectId, tabSegment: parts[2] });
    }
    default:
      // #/notifications, #/board and anything unknown: the inbox is the landing
      // surface, so it is also the fallback.
      return inbox();
  }
}

export function hashFromRoute(route) {
  const encode = encodeURIComponent;
  if (route.name === "branch" && route.projectId && route.branch) {
    return `#/project/${encode(route.projectId)}/branch/${encode(route.branch)}/${branchTab(route.tab)}`;
  }
  if (route.name === "issue" && route.projectId && route.id) {
    const base = `#/project/${encode(route.projectId)}/issue/${encode(route.id)}`;
    return route.stage ? `${base}/stage/${encode(route.stage)}` : base;
  }
  if (route.name === "account") return `#/account/${ACCOUNT_PAGES.has(route.page) ? route.page : "settings"}`;
  return "#/inbox";
}
