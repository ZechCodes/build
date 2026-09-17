// Hash routes for the three-panel shell. There are two work items and two
// global surfaces, and that is the whole vocabulary:
//   #/inbox                                     — the landing surface
//   #/project/<projectId>                       — the project: its workspaces
//   #/project/<projectId>/workspace/<id>[/directory/<sourceId>]/<tab>
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
// know (a run id, a worktree id, an issue with no project in the URL). Those
// parse to a `resolve` route: the app looks the id up in the feed
// (core/routeResolve.js) and rewrites the hash.
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

/** The inbox, standing in one project: the rail with that project's block
 *  marked. It has a URL of its own (`#/project/<id>/inbox`) so the landing is
 *  stable — the hash the app rewrites to parses back to the same route rather
 *  than losing the project on the way. */
const inboxIn = (projectId) => (projectId ? { name: "inbox", projectId } : inbox());

/**
 * The project's own page: its workspaces in the main pane, its agent in the rail.
 *
 * A project is a template — the base checkout is what workspaces are cut FROM,
 * and it is not a place to work — so this page is about the project and never
 * about that checkout. Every URL that used to open the checkout
 * (`#/project/<id>` and the retired tabs that hung off it) lands here.
 */
const projectPage = (projectId) => (projectId ? { name: "project", projectId } : inbox());

/** `<workspaceId>[/directory/<sourceId>][/<tab>]`. The directory is optional,
 *  so the tab is whichever segment follows whatever came before it. */
function workspaceRoute(projectId, parts) {
  if (!parts[0]) return projectPage(projectId);
  const [sourceId, tabSegment] = parts[1] === "directory" ? [parts[2], parts[3]] : [undefined, parts[1]];
  return {
    name: "workspace",
    projectId,
    workspaceId: parts[0],
    ...(sourceId ? { sourceId } : null),
    tab: branchTab(tabSegment),
  };
}

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
  if (!tailSegments.length) return projectPage(projectId);
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

/**
 * A URL that names a project and nothing inside it, plus every tab that used to
 * hang off its base checkout.
 *
 * The checkout has no surface of its own any more, so they all land on the
 * project's page. The exception is the retired right-cluster tabs, which named
 * a project-wide pane rather than the checkout: Archive has a surface of its own
 * to go to, and the two that named the inbox go to the rail standing here.
 */
function projectSurface(projectId, tabSegment) {
  if (!projectId) return inbox();
  const cluster = clusterRoute(tabSegment);
  if (cluster) return cluster.name === "inbox" ? inboxIn(projectId) : cluster;
  return projectPage(projectId);
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

// The surfaces whose Files tab stands in a file: both are a checkout with a
// tree, and both carry the file as a query rather than a path segment.
const FILE_TAB_SURFACES = new Set(["branch", "workspace"]);

/// The route a hash names, and where in it the reader is standing.
///
/// Split before parse: everything up to the `?` is the surface, everything
/// after it is the place within it.
export function routeFromHash(hash) {
  const [path, query] = String(hash || "").split("?");
  const route = surfaceFromHashPath(path);
  const place = FILE_TAB_SURFACES.has(route.name) && route.tab === "files" ? tabPlace(query) : null;
  // The place is merged BEFORE the device question is asked: a device-less
  // Files link parks on a resolve route that still knows which file it meant.
  return withDeviceOrResolve(place ? { ...route, ...place } : route);
}

/// The segments of a hash path, decoded, with the empties dropped.
const segmentsOf = (hash) =>
  (hash || "")
    .replace(/^#\/?/, "")
    .split("/")
    .filter(Boolean)
    .map(decodeURIComponent);

/// The device a URL names, taken off the front so the parser below reads the
/// same segments it always has. Only a `project` URL can carry one —
/// `#/device/<d>` and `#/device/<d>/settings` are the device's own page.
const peelDevice = (parts) =>
  parts[0] === "device" && parts[2] === "project"
    ? { deviceId: parts[1], rest: parts.slice(2) }
    : { deviceId: null, rest: parts };

/// Which machine the route is about. A route that names no project names no
/// machine either — the inbox and the account pages are the whole account's.
const stampDevice = (route, deviceId) => (deviceId && route.projectId ? { ...route, deviceId } : route);

function surfaceFromHashPath(hash) {
  const { deviceId, rest } = peelDevice(segmentsOf(hash));
  return stampDevice(surfaceFromSegments(rest), deviceId);
}

/** The collections under a project that address a legacy id, and what the app
 *  has to look that id up as. */
const LEGACY_ID_COLLECTIONS = Object.freeze({ task: "run", worktree: "worktree" });

/**
 * Everything under `#/project/<id>`: the collections that name something inside
 * the project, and the project's own page for everything that names nothing.
 *
 * `parts` is the tail after the project id. A collection with nothing named in
 * it — `…/plan`, `…/task` — says the project and no more, so it lands where
 * every other project-and-no-more URL lands.
 */
function insideProject(projectId, parts) {
  const [collection, id, ...tail] = parts;
  if (collection === "workspace") return workspaceRoute(projectId, [id, ...tail]);
  if (collection === "branch") return branchRoute(projectId, id ? [id, ...tail] : []);
  if (!id) return projectSurface(projectId, collection);
  if (collection === "issue" || collection === "plan") return issueRoute(projectId, id, tail);
  const kind = LEGACY_ID_COLLECTIONS[collection];
  return kind ? resolveRoute(kind, { projectId, id, tabSegment: tail[0] }) : projectSurface(projectId, collection);
}

// eslint-disable-next-line complexity -- ratchet: surfaceFromSegments is at 19, cap 10 — reduce it, then drop this line
function surfaceFromSegments(parts) {
  switch (parts[0]) {
    case "device":
      return parts[1] ? { name: "device", id: parts[1] } : inbox();
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
    case "project":
      return parts[1] ? insideProject(parts[1], parts.slice(2)) : inbox();
    default:
      // #/notifications, #/board and anything unknown: the inbox is the landing
      // surface, so it is also the fallback.
      return inbox();
  }
}

const encode = encodeURIComponent;

/// The `?path=…&line=…` a Files route ends with, and nothing at all for every
/// other tab: only Files stands in a file.
function tabPlaceSuffix(route, tab) {
  if (tab !== "files" || !route.file) return "";
  const query = new URLSearchParams({ path: route.file });
  if (Number.isFinite(route.line) && route.line > 0) query.set("line", String(route.line));
  return `?${query}`;
}

/// Everything a work URL says before the branch or the issue: the machine the
/// project is on, when the route names one, and the project itself. A route
/// that names no device is written without one — a device is never invented.
function projectPrefix(route) {
  const project = `project/${encode(route.projectId)}`;
  return route.deviceId ? `#/device/${encode(route.deviceId)}/${project}` : `#/${project}`;
}

// The surfaces that are about one machine's checkout, and so cannot be opened
// until the route says which machine: every device mints a `proj-1`.
const WORK_SURFACES = new Set(["branch", "issue", "project", "workspace"]);

/// A work route that names no machine is a question, not a destination: park it
/// on the resolve route that asks the feed which device holds that project, and
/// keep the route it meant (and the terminal it named) for the answer.
export function withDeviceOrResolve(route) {
  if (!route || route.deviceId || !route.projectId || !WORK_SURFACES.has(route.name)) return route;
  return { name: "resolve", kind: "project", projectId: route.projectId, route, ...termOf(route.term) };
}

/// How each kind of route is written, one writer per kind. A writer answers
/// null when the route is missing what its URL is made of — an unwritable route
/// has no link of its own, and the inbox is where the app lands without one.
/// The `resolve` routes are the same: a question has no URL, only the URL that
/// asked it (see withDeviceOrResolve).
const HASH_WRITERS = Object.freeze({
  // The rail standing in one project has a URL because the app rewrites the
  // hash to whatever the route it took up writes: without one, every project
  // link would land scoped and then immediately re-parse as the whole
  // account's inbox. The plain inbox writes nothing and takes the fallback.
  inbox: (route) => (route.projectId ? `${projectPrefix(route)}/inbox` : null),
  // The project's own page is the project and nothing after it.
  project: (route) => (route.projectId ? projectPrefix(route) : null),
  workspace: (route) => {
    if (!route.projectId || !route.workspaceId) return null;
    const tab = branchTab(route.tab);
    const source = route.sourceId ? `/directory/${encode(route.sourceId)}` : "";
    return `${projectPrefix(route)}/workspace/${encode(route.workspaceId)}${source}/${tab}${tabPlaceSuffix(route, tab)}`;
  },
  branch: (route) => {
    if (!route.projectId || !route.branch) return null;
    const tab = branchTab(route.tab);
    return `${projectPrefix(route)}/branch/${encode(route.branch)}/${tab}${tabPlaceSuffix(route, tab)}`;
  },
  issue: (route) => {
    if (!route.projectId || !route.id) return null;
    const base = `${projectPrefix(route)}/issue/${encode(route.id)}`;
    return route.stage ? `${base}/stage/${encode(route.stage)}` : base;
  },
  device: (route) => (route.id ? `#/device/${encode(route.id)}/settings` : null),
  capture: (route) => (route.id ? `#/capture/${encode(route.id)}` : null),
  account: (route) => `#/account/${ACCOUNT_PAGES.has(route.page) ? route.page : "settings"}`,
});

export function hashFromRoute(route) {
  const write = HASH_WRITERS[route.name];
  return (write && write(route)) || "#/inbox";
}
