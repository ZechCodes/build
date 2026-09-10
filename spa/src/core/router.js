// Hash routes for the three-panel shell. There are two work items and two
// global surfaces, and that is the whole vocabulary:
//   #/inbox                                     — the landing surface
//   #/project/<projectId>/branch/<name>/<tab>   — tab is changes | files
//   #/project/<projectId>/issue/<issueId>[/stage/<stageId>]
//   #/capture/<captureId>                       — what to do with a capture
//   #/account[/<page>]                          — page is settings | devices | archive
//
// Every URL the pre-redesign client could mint still opens the nearest of
// those. Conversation and Agent are the agent rail now, terminals are the
// console, and Diff merged into Changes, so those tabs all fold into Changes;
// the old right-cluster tabs (Inbox, Issues, Archive) named project-wide panes
// and go to the global surface that owns them. A `term-<n>` tab lands on
// Changes too, and carries the terminal it named as `term`: the console opens
// on it (core/consoleModel.js), which is where that tab now lives.
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

// A terminal tab named a terminal, and that outlived the tab: the surface it
// opens is the branch, with the console open on it.
const termOf = (segment) => (isTermTab(segment) ? { term: segment } : null);

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
    ...(trailingTab ? termOf(last) : null),
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
  return { ...route, ...termOf(tabSegment) };
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

/// Where in a tab the URL is standing, as a query on the end of the hash.
///
/// The Files tab names a FILE, and a file path carries slashes exactly as a
/// branch name does — two slashed things in one path cannot be told apart, and
/// the branch/tab split is already delicate enough. So the file rides as
/// `?path=…&line=…` and the path parsing above never sees it.
function tabPlace(query) {
  if (!query) return null;
  const params = new URLSearchParams(query);
  const path = params.get("path");
  if (!path) return null;
  const line = Number(params.get("line"));
  return { file: path, ...(Number.isFinite(line) && line > 0 ? { line } : null) };
}

/// The route a hash names, and where in it the reader is standing.
///
/// Split before parse: everything up to the `?` is the surface, everything
/// after it is the place within it.
export function routeFromHash(hash) {
  const [path, query] = String(hash || "").split("?");
  const route = surfaceFromHashPath(path);
  const place = route.name === "branch" && route.tab === "files" ? tabPlace(query) : null;
  return place ? { ...route, ...place } : route;
}

// eslint-disable-next-line complexity -- ratchet: surfaceFromHashPath is at 28, cap 10 — reduce it, then drop this line
function surfaceFromHashPath(hash) {
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
    case "capture":
      // A capture belongs to no project until something routes it, so the
      // decision page is named by the capture and nothing else.
      return parts[1] ? { name: "capture", id: parts[1] } : inbox();
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

/// The `?path=…&line=…` a Files route ends with, and nothing at all for every
/// other tab: only Files stands in a file.
function tabPlaceSuffix(route, tab) {
  if (tab !== "files" || !route.file) return "";
  const query = new URLSearchParams({ path: route.file });
  if (Number.isFinite(route.line) && route.line > 0) query.set("line", String(route.line));
  return `?${query}`;
}

// eslint-disable-next-line complexity -- ratchet: hashFromRoute is at 12, cap 10 — reduce it, then drop this line
export function hashFromRoute(route) {
  const encode = encodeURIComponent;
  if (route.name === "branch" && route.projectId && route.branch) {
    const tab = branchTab(route.tab);
    return `#/project/${encode(route.projectId)}/branch/${encode(route.branch)}/${tab}${tabPlaceSuffix(route, tab)}`;
  }
  if (route.name === "issue" && route.projectId && route.id) {
    const base = `#/project/${encode(route.projectId)}/issue/${encode(route.id)}`;
    return route.stage ? `${base}/stage/${encode(route.stage)}` : base;
  }
  if (route.name === "capture" && route.id) return `#/capture/${encode(route.id)}`;
  if (route.name === "account") return `#/account/${ACCOUNT_PAGES.has(route.page) ? route.page : "settings"}`;
  return "#/inbox";
}
