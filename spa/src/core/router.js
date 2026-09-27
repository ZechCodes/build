// Hash routes for the three-panel shell. There are two work items and two
// global surfaces, and that is the whole vocabulary:
//   #/inbox                                     — the landing surface
//   #/project/<projectId>[?view=list|board]     — the project: its task tracker
//   #/project/<projectId>/workspaces            — the project: its workspaces
//   #/project/<projectId>/tasks[?view=board]    — an alias for the tracker
//   #/project/<projectId>/tasks/<taskId>[/c/<commentId>]
//   #/project/<projectId>/workspace/<id>[/directory/<sourceId>]/<tab>[?commit=<sha>]
//   #/project/<projectId>/branch/<name>/<tab>   — tab is changes | files
//   #/project/<projectId>/plan/<planId>[/stage/<stageId>]
//   #/capture/<captureId>                       — what to do with a capture
//   #/account[/<page>]                          — page is settings | devices | archive
//
// Every URL the pre-redesign client could mint still opens the nearest of
// those. Conversation and Agent are the agent rail now, terminals are the
// console, and Diff merged into Changes, so those tabs all fold into Changes;
// the old right-cluster tabs (Inbox, Tasks, Archive) named project-wide panes
// and go to the global surface that owns them. A `term-<n>` tab lands on
// Changes too, and carries the terminal it named as `term`: the console opens
// on it (core/consoleModel.js), which is where that tab now lives.
//
// Some legacy URLs name an entity by an id whose branch this module cannot
// know (a run id, a worktree id, a task with no project in the URL). Those
// parse to a `resolve` route: the app looks the id up in the feed
// (core/routeResolve.js) and rewrites the hash.
//
// Pure mapping both ways; the app shell owns the hashchange listener.

const BRANCH_TABS = new Set(["changes", "files"]);
const ACCOUNT_PAGES = new Set(["settings", "devices", "archive"]);

/// The project page's second tab, and the collection the tracker lives under.
/// One word for both, because `#/project/<p>/tasks` IS that tab.
const TASKS_TAB = "tasks";

/// What links written before tasks were renamed (#190) still say. They parse
/// to where they always went and nothing writes them: the tracker and the
/// workspace tab were `issues`, and a retired plan was `issue` — never `task`,
/// which a run's legacy URL already held.
const LEGACY_TASKS_TAB = "issues";
const LEGACY_PLAN_COLLECTION = "issue";
const isTasksSegment = (segment) => segment === TASKS_TAB || segment === LEGACY_TASKS_TAB;

/// The project page's other tab. Tasks is first and is what a project link
/// opens on (#46), so Tasks writes nothing and this one names itself: a URL
/// says only what is not the default. `#/project/<p>` IS the Tasks tab, and
/// every `/tasks` link written before the flip still parses to the same place.
const WORKSPACES_TAB = "workspaces";

/// Which way the Tasks tab is laid out. Dashboard is the project's default;
/// the workspace Tasks tab still opens on its list.
const DASHBOARD_VIEW = "dashboard";
const LIST_VIEW = "list";
const BOARD_VIEW = "board";

const isTermTab = (segment) => /^term-\d+$/.test(segment || "");
// Tab segments a pre-redesign URL could carry. They are not destinations any
// more, but they still have to be RECOGNIZED as tabs — otherwise a trailing
// `conversation` would read as part of a slashed branch name.
const RETIRED_TABS = new Set(["conversation", "agent", "stages", "diff", "plan", "review", "inbox", LEGACY_TASKS_TAB, "archive"]);
const isTabSegment = (segment) => BRANCH_TABS.has(segment) || RETIRED_TABS.has(segment) || isTermTab(segment);

// The retired tabs that named a project-wide pane rather than the entity's own
// work surface: whichever entity carried them, they belong to a global route.
const clusterRoute = (segment) =>
  segment === "inbox" || segment === LEGACY_TASKS_TAB
    ? { name: "inbox" }
    : segment === "archive"
      ? { name: "account", page: "archive" }
      : null;

// Files is the one entity tab that kept its name; everything else lands on
// Changes, including nothing at all.
const branchTab = (segment) => (BRANCH_TABS.has(segment) ? segment : "changes");

/** A WORKSPACE's tabs: the checkout's two faces, plus the tasks its agents
 *  hold (#29). A branch has neither agents nor tasks, so it keeps `branchTab`
 *  and the old `issues` there stays the retired right-cluster tab that named
 *  the inbox. */
const workspaceTab = (segment) => (isTasksSegment(segment) ? TASKS_TAB : branchTab(segment));

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
const projectPage = (projectId, tab = TASKS_TAB) => (projectId ? { name: "project", projectId, tab } : inbox());

/**
 * `<workspaceId>[/directory/<sourceId>][/<tab>[/<taskId>]]`.
 *
 * The directory is optional, so the tab is whichever segment follows whatever
 * came before it — and on the tasks tab one more segment may follow, naming
 * the task open inside it.
 *
 * All of it stays `name: "workspace"`. That is not tidiness: the shell keys the
 * rail on the route's name and its workspace id (core/shell.js), so a route
 * that stayed a workspace keeps the same bubbles standing across a directory
 * tab, the tasks tab and one task inside it — which is the whole of what the
 * reader sees when the page swaps under them.
 */
function workspaceRoute(projectId, parts) {
  if (!parts[0]) return projectPage(projectId);
  const scoped = parts[1] === "directory";
  const [sourceId, tabSegment, tail] = scoped ? [parts[2], parts[3], parts[4]] : [undefined, parts[1], parts[2]];
  const tab = workspaceTab(tabSegment);
  return {
    name: "workspace",
    projectId,
    workspaceId: parts[0],
    ...(sourceId ? { sourceId } : null),
    tab,
    // Only the tasks tab has anything after it. A stray segment behind a
    // directory tab named nothing before this and still names nothing.
    ...(tab === TASKS_TAB && tail ? { taskId: tail } : null),
  };
}

/** A legacy stage deep-link: `<tab>/<stageId>` where the tab is one of the
 *  retired plan tabs (stages, review, conversation…). Returns the stage id, or
 *  undefined when the segments name no stage. */
function legacyStage(tabSegment, stageSegment) {
  if (!stageSegment) return undefined;
  return tabSegment === "stage" || isTabSegment(tabSegment) ? stageSegment : undefined;
}

/**
 * `#/project/<p>/tasks[/<taskId>]` — the tracker.
 *
 * With an id it is one task's page; without one it is the project page
 * standing on its Tasks tab. That bare URL used to park on the inbox, as one
 * of the retired right-cluster tabs: the old Tasks tab named a project-wide
 * task pane, and there is a project-wide task pane again, so it lands on it.
 * The same segment on a BRANCH or a legacy task still goes to the inbox —
 * there is no tracker of a branch to open.
 */
const trackerRoute = (projectId, taskId, tail) =>
  taskId
    ? { name: "trackerTask", projectId, taskId, ...(tail[0] === "c" && tail[1] ? { commentId: tail[1] } : null) }
    : projectPage(projectId);

/** `#/project/<p>/task/<id>[…]` — the project is in the URL, so this is the
 *  canonical task route no matter which legacy tail follows the id. */
function taskRoute(projectId, id, tailSegments) {
  const cluster = clusterRoute(tailSegments[0]);
  if (cluster) return cluster;
  const route = { name: "task", projectId, id };
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

/** A legacy task URL with no project in it: same parking spot, but tasks have
 *  stages instead of tabs. */
function resolveTaskRoute(id, tailSegments) {
  const cluster = clusterRoute(tailSegments[0]);
  if (cluster) return cluster;
  const route = { name: "resolve", kind: "task", id };
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

// The surfaces whose rail can be opened on a named conversation: the two pages
// an agent belongs to. Which agent the rail stands on is not part of the
// surface — the page is the same page — so it rides as a query beside the file.
const AGENT_SURFACES = new Set(["workspace", "project"]);

// The surfaces with a Tasks tab of their own: the project's whole tracker,
// and the subset a workspace's agents hold (#29).
const TASKS_SURFACES = new Set(["project", "workspace"]);

/// The agent a URL names, or null for one that names none.
function railAgent(query) {
  const params = new URLSearchParams(query || "");
  const agent = params.get("agent");
  if (agent) return { agent };
  return params.get("newAgent") === "1" ? { newAgent: true } : null;
}

/// A project URL omits Dashboard, while a workspace URL omits List. The other
/// modes name themselves so a reload returns to the same layout.
function tasksView(query, route) {
  const view = query ? new URLSearchParams(query).get("view") : "";
  if (view === BOARD_VIEW) return { view: BOARD_VIEW };
  if (route.name === "project" && view === LIST_VIEW) return { view: LIST_VIEW };
  if (route.name === "workspace" && view === DASHBOARD_VIEW) return { view: DASHBOARD_VIEW };
  return null;
}

/// Everything a surface reads off the query rather than off the path: where in
/// a tab the reader is standing, whose conversation the rail is open on, and
/// which way the Tasks tab is laid out. A surface that reads none gets null,
/// and its route is left exactly as the path made it.
function placeInSurface(route, query) {
  const place = FILE_TAB_SURFACES.has(route.name) && route.tab === "files" ? tabPlace(query) : null;
  const commit = commitPlace(route, query);
  const agent = AGENT_SURFACES.has(route.name) ? railAgent(query) : null;
  // Both surfaces that HAVE a tasks tab read it: the project's own, and a
  // workspace's (#29). They are the same tracker laid out the same two ways.
  const view = TASKS_SURFACES.has(route.name) && route.tab === TASKS_TAB ? tasksView(query, route) : null;
  const position = { ...place, ...agent, ...view, ...commit };
  return Object.keys(position).length ? position : null;
}

function commitPlace(route, query) {
  if (route.name !== "workspace" || route.tab !== "changes" || !query) return null;
  const commit = new URLSearchParams(query).get("commit");
  return commit ? { commit } : null;
}

/// The route a hash names, and where in it the reader is standing.
///
/// Split before parse: everything up to the `?` is the surface, everything
/// after it is the place within it.
export function routeFromHash(hash) {
  const [path, query] = String(hash || "").split("?");
  const route = surfaceFromHashPath(path);
  // Merged BEFORE the device question is asked: a device-less Files link parks
  // on a resolve route that still knows which file it meant, a device-less
  // conversation link one that still knows which agent, and a device-less board
  // link one that still knows it was a board.
  const place = placeInSurface(route, query);
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
  if (isTasksSegment(collection)) return trackerRoute(projectId, id, tail);
  if (collection === WORKSPACES_TAB) return projectPage(projectId, WORKSPACES_TAB);
  if (!id) return projectSurface(projectId, collection);
  if (collection === LEGACY_PLAN_COLLECTION || collection === "plan") return taskRoute(projectId, id, tail);
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
    case LEGACY_PLAN_COLLECTION:
    case "plan":
      if (!parts[1]) return inbox();
      return resolveTaskRoute(parts[1], parts.slice(2));
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

/// The query a hash ends with, out of the pairs its surface has to say and
/// nothing for the ones it has not: where in a tab the reader is standing, and
/// whose conversation the rail is open on. One `?`, so one writer puts both
/// there.
function hashQuery(pairs) {
  const query = new URLSearchParams(pairs.filter(([, value]) => value));
  return String(query) ? `?${query}` : "";
}

/// The `path` and `line` a Files route ends with, and nothing at all for every
/// other tab: only Files stands in a file.
const tabPlacePairs = (route, tab) =>
  tab === "files" && route.file
    ? [["path", route.file], ["line", Number.isFinite(route.line) && route.line > 0 ? String(route.line) : ""]]
    : [];

/// The conversation the rail is standing on, where the route names one.
const railAgentPairs = (route) => [["agent", route.agent || ""], ["newAgent", route.newAgent ? "1" : ""]];

/// The Tasks tab's layout, omitting its own default on each surface.
const tasksViewPairs = (route, standing = TASKS_TAB) => {
  if (route.tab !== standing) return [["view", ""]];
  if (route.view === BOARD_VIEW) return [["view", BOARD_VIEW]];
  if (route.name === "project" && route.view === LIST_VIEW) return [["view", LIST_VIEW]];
  if (route.name === "workspace" && route.view === DASHBOARD_VIEW) return [["view", DASHBOARD_VIEW]];
  return [["view", ""]];
};

/// Which of the project page's two tabs a route stands on. Tasks is the
/// default (#46), so it writes nothing and a bare project link opens it;
/// Workspaces is the one that has to name itself.
const projectTabPath = (route) => (route.tab === WORKSPACES_TAB ? `/${WORKSPACES_TAB}` : "");

/// Everything a work URL says before the branch or the task: the machine the
/// project is on, when the route names one, and the project itself. A route
/// that names no device is written without one — a device is never invented.
function projectPrefix(route) {
  const project = `project/${encode(route.projectId)}`;
  return route.deviceId ? `#/device/${encode(route.deviceId)}/${project}` : `#/${project}`;
}

// The surfaces that are about one machine's checkout, and so cannot be opened
// until the route says which machine: every device mints a `proj-1`.
const WORK_SURFACES = new Set(["branch", "task", "project", "trackerTask", "workspace"]);

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
  // The project's own page is the project, the tab it stands on when that is
  // not the workspaces it opens on, and the query each of those two reads.
  project: (route) =>
    route.projectId
      ? `${projectPrefix(route)}${projectTabPath(route)}${hashQuery([...railAgentPairs(route), ...tasksViewPairs(route)])}`
      : null,
  // One task of the tracker, under the project it belongs to and never moves
  // between.
  trackerTask: (route) =>
    route.projectId && route.taskId
      ? `${projectPrefix(route)}/${TASKS_TAB}/${encode(route.taskId)}${route.commentId ? `/c/${encode(route.commentId)}` : ""}`
      : null,
  workspace: (route) => {
    if (!route.projectId || !route.workspaceId) return null;
    const tab = workspaceTab(route.tab);
    const tasks = tab === TASKS_TAB;
    const source = route.sourceId ? `/directory/${encode(route.sourceId)}` : "";
    // One task open inside the tab, and the board when that is not the list
    // it opens on — the same two things the project's own Tasks tab writes.
    const opened = tasks && route.taskId ? `/${encode(route.taskId)}` : "";
    const place = tasks ? tasksViewPairs(route, TASKS_TAB)
      : tab === "changes" ? [["commit", route.commit || ""]] : tabPlacePairs(route, tab);
    const query = hashQuery([...place, ...railAgentPairs(route)]);
    return `${projectPrefix(route)}/workspace/${encode(route.workspaceId)}${source}/${tab}${opened}${query}`;
  },
  branch: (route) => {
    if (!route.projectId || !route.branch) return null;
    const tab = branchTab(route.tab);
    return `${projectPrefix(route)}/branch/${encode(route.branch)}/${tab}${hashQuery(tabPlacePairs(route, tab))}`;
  },
  task: (route) => {
    if (!route.projectId || !route.id) return null;
    const base = `${projectPrefix(route)}/plan/${encode(route.id)}`;
    return route.stage ? `${base}/stage/${encode(route.stage)}` : base;
  },
  device: (route) => (route.id ? `#/device/${encode(route.id)}/settings` : null),
  capture: (route) => (route.id ? `#/capture/${encode(route.id)}` : null),
  account: (route) => `#/account/${ACCOUNT_PAGES.has(route.page) ? route.page : "settings"}`,
});

/**
 * The route that opens one agent's conversation: the page the agent belongs to,
 * with the rail standing on it.
 *
 * What a link from a message to "the conversation it came from" is written
 * from, so it is a route rather than a hash — the app navigates with routes,
 * and `hashFromRoute` is what turns one into a URL. Nothing is invented: a
 * caller that names no device or no agent gets a route that names none either,
 * which opens the same page the way every other link to it does.
 */
export function conversationRoute({ kind, projectId, deviceId = null, workspaceId = null, agentId = null } = {}) {
  const page = {
    projectId,
    ...(deviceId ? { deviceId } : null),
    ...(agentId ? { agent: agentId } : null),
  };
  if (kind !== "workspace") return { name: "project", ...page };
  return { name: "workspace", ...page, workspaceId, tab: "changes" };
}

export function hashFromRoute(route) {
  const write = HASH_WRITERS[route.name];
  return (write && write(route)) || "#/inbox";
}
