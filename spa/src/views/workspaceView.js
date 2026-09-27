// A workspace is a durable root containing one or more registered source
// directories. Directory selection scopes Files and Changes; the console stays
// scoped to the workspace, so navigating between directories or refs never
// replaces its server sessions. The rail down the left edge is the workspace's
// navigation — Changes, Files, Tasks, Settings (core/workspaceRail.js) — and
// stands on the route alone.

import { $ } from "../dom.js";
import { App, go, markRoute } from "../app.js";
import { esc } from "../core/text.js";
import { mountWorkspaceRail } from "../core/workspaceRail.js";
import { shellSelection } from "../core/shell.js";
import { renderFilesTab } from "./files.js";
import { directoryId, selectedDirectory, workspaceDirectoryModel, workspaceScope } from "../core/workspaceModel.js";
import { workspaceLayoutCacheId } from "../core/directoryScope.js";
import { mountWorkspaceChanges } from "./workspaceChanges.js";
import { mountWorkspaceGitInitialization } from "../core/workspaceGitInitialization.js";
import { routeContext } from "../core/deviceContexts.js";
import { surfaceContext } from "../core/surfaceContext.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { deviceFeedNow } from "../core/feedRows.js";
import { cachedFeedView } from "../core/cachedRows.js";
import { mergeCached, readCached, subscribeCache, writeCached } from "../core/localCache.js";
import { upsertSessionRow } from "../core/sessionListCache.js";
import { refreshFeed, subscribeFeed } from "../core/taskFeed.js";
import { mountWorkspaceTasksTab, workspaceTasksPlace } from "../core/workspaceTasksTab.js";
import { taskContextItem } from "../core/trackerViewingContext.js";
import "../styles/tasks.css";
import "../styles/surfaces.css";

/** The workspace's third tab, which is not about a directory at all: the
 *  tasks its own agents are holding (#29). A directory scopes Changes and
 *  Files; it does not scope this. */
const TASKS_TAB = "tasks";
const onTasksTab = (route) => route.tab === TASKS_TAB;

function errorHtml(message) {
  return `<div class="empty"><h2>Workspace unavailable</h2><p>${esc(message)}</p></div>`;
}

const workspaceListAddress = (state) =>
  state.context.cacheScope.address({ entityId: "", kind: "workspaces" });

const gitOptionsAddress = (state, sourceId) =>
  state.context.cacheScope.address({ entityId: state.route.workspaceId, kind: "git-init-options", sub: sourceId });

async function writeWorkspaceResult(state, workspace) {
  const address = workspaceListAddress(state);
  if (!address || !workspace) return;
  await upsertSessionRow(address, "workspaces", workspace, () => routeHoldsWorkspace(state));
  await state.workspaceRead;
}

const routeHoldsWorkspace = (state) =>
  ownsWorkspace(state) && App.route.workspaceId === state.route.workspaceId;

const selectedNeedsRetryRefresh = (state) =>
  state.retryRefreshPending && selectedDirectory(state.workspace, state.sourceId)?.status !== "ready";

/** What this view knows of one directory's Git initialization: every directory
 *  Changes has shown keeps its own, so going back to one reads nothing again. */
function gitInitOf(state, sourceId) {
  if (!state.gitInit.has(sourceId)) {
    state.gitInit.set(sourceId, { sourceGit: null, sourceNeedsReconciliation: false, workspaceNeedsReconciliation: false, painted: false, probePending: false, needsHost: false });
  }
  return state.gitInit.get(sourceId);
}

async function cachedWorkspaceResult(state) {
  const address = workspaceListAddress(state);
  const workspaces = address ? (await readCached(address))?.value : null;
  return Array.isArray(workspaces)
    ? workspaces.find((row) => (row.workspace_id || row.id) === state.route.workspaceId)
    : null;
}

async function readWorkspaceResult(state) {
  const workspace = await cachedWorkspaceResult(state);
  if (!routeHoldsWorkspace(state)) return;
  if (!workspace || !state.workspace) return;
  const refresh = selectedNeedsRetryRefresh(state);
  state.workspace = workspace;
  state.retryRefreshPending = false;
  if (refresh) state.refreshPane?.(workspace);
  // Changes redraws its tab row, and turns a directory's offer into its commit
  // rail once the record says it has git — the view is not built again.
  else state.pane?.workspaceMoved?.(workspace);
  state.paintTabs?.();
  state.onWorkspaceCached?.(workspace);
}

async function writeGitResult(state, sourceId, answer) {
  const address = gitOptionsAddress(state, sourceId);
  if (!address || !workspaceHoldsSource(state, sourceId)) return;
  const directory = answer.workspace?.directories?.find((entry) => directoryId(entry) === sourceId);
  const completed = new Set((answer.results || answer.outcomes || []).filter((result) => result.status !== "failed").map((result) => result.target));
  await mergeCached(address, (held) => ({
    ...held,
    workspace_id: state.route.workspaceId,
    source_id: sourceId,
    workspace: {
      ...held?.workspace,
      ...(directory && { is_git: directory.is_git }),
      ...(completed.has("workspace") || completed.has("both") ? { needs_reconciliation: false } : null),
    },
    source: {
      ...held?.source,
      ...(answer.source && typeof answer.source.is_git === "boolean" ? { is_git: answer.source.is_git } : null),
      ...(completed.has("source") || completed.has("both") ? { needs_reconciliation: false } : null),
    },
  }));
  await state.gitOptionsRead;
}

async function readGitOptions(state, sourceId) {
  const address = gitOptionsAddress(state, sourceId);
  if (!address) return;
  const options = (await readCached(address))?.value;
  if (!options || !workspaceHoldsSource(state, sourceId)) return;
  const git = gitInitOf(state, sourceId);
  git.sourceGit = options.source?.is_git !== false;
  git.sourceNeedsReconciliation = options.source?.needs_reconciliation === true;
  git.workspaceNeedsReconciliation = options.workspace?.needs_reconciliation === true;
  state.paintTabs?.();
}

/** A failed workspace's Retry is one action, however many kept surfaces carry
 *  its button: each directory Changes has shown holds a host of its own, and
 *  every one of them says the same thing — pending, what came of it — so the
 *  one showing is never behind the others, and a second press while one is
 *  asking asks nothing. */
function installWorkspaceAction(state, workspace) {
  if (workspace?.status !== "failed") return;
  const retry = { current: workspace, pending: false, message: "", failed: false };
  const hosts = new Set();
  const paintAll = () => {
    for (const host of hosts) {
      if (host.isConnected) paintRetry(host, retry, submit);
      else hosts.delete(host);
    }
  };
  const submit = () => submitRetry(state, retry, paintAll);
  state.onWorkspaceCached = (workspaceRow) => {
    retry.current = workspaceRow;
    paintAll();
  };
  // On the surface, where the Git offer hangs (paintWorkspaceAction) — the bar
  // over a workspace is its picker and nothing else (#174).
  state.workspaceAction = (host) => {
    hosts.add(host);
    paintRetry(host, retry, submit);
  };
}

function paintRetry(host, retry, submit) {
  const status = `<span class="workspace-action-status${retry.failed ? " error" : ""}" role="status">${esc(retry.message)}</span>`;
  if (retry.current?.status !== "failed") {
    host.innerHTML = status;
    return;
  }
  host.innerHTML = `${status}
      <button class="btn mini" type="button" data-workspace-action${retry.pending ? " disabled" : ""}>Retry</button>`;
  host.querySelector("[data-workspace-action]").onclick = submit;
}

async function submitRetry(state, retry, paintAll) {
  if (retry.pending) return;
  Object.assign(retry, { pending: true, message: "", failed: false });
  paintAll();
  try {
    const answer = await state.callRpc("workspace.retry", { workspace_id: state.route.workspaceId });
    if (state.disposed) return;
    state.retryRefreshPending = true;
    await writeWorkspaceResult(state, answer.workspace || answer);
    retry.message = retry.current?.status === "ready" ? "Workspace ready." : "Retry finished.";
  } catch (error) {
    if (state.disposed) return;
    retry.failed = true;
    retry.message = error.message || String(error);
  } finally {
    state.retryRefreshPending = false;
    retry.pending = false;
    if (!state.disposed) paintAll();
  }
}

/** A failed workspace's Retry, and what came of it, where Changes keeps its
 *  offers: just before the Git offer where there is one — at the foot of a
 *  commit rail, under the sentence on a directory without Git. */
function paintWorkspaceAction(list, state) {
  if (!state.workspaceAction || list.querySelector(".workspace-action-host")) return;
  const host = document.createElement("div");
  host.className = "workspace-action-host";
  list.insertBefore(host, list.querySelector(":scope > .workspace-init-host"));
  state.workspaceAction(host);
}

function mountDirectoryPane(body, options) {
  const { canonical } = options;
  if (onTasksTab(canonical)) return mountTasksTab(body, options);
  return mountCheckoutPane(body, options);
}

/**
 * Say which task is on screen, so the agent beside it knows what the reader
 * is looking at (#21, #29).
 *
 * The WORKSPACE half of the stamp is already there: the rail is standing on
 * this workspace, so anything sent from here to the project's agent wears the
 * workspace item (core/agentRail.js). This adds the task to it, from the READ
 * rather than from the route — an agent told an id and nothing else is no
 * better off — and only where the bridge takes the kind at all.
 */
const sayWhichTask = (deviceId) => (task) => {
  const item = taskContextItem(task, deviceId);
  if (item) App.viewingContext?.set?.({ version: 1, items: [item] });
};

function mountTasksTab(body, { canonical, callRpc, context, feed, agentSelection }) {
  return mountWorkspaceTasksTab(body, {
    route: canonical,
    context: context || { deviceId: canonical.deviceId, rpc: callRpc },
    feed,
    selection: agentSelection,
    sayWhichTask: sayWhichTask(canonical.deviceId),
    navigate: go,
  });
}

/** The workspace's directories as the Files tree's roots, in its own order. */
const filesRoots = (workspace, workspaceId) =>
  workspaceDirectoryModel(workspace).map((directory) => ({
    id: directory.sourceId,
    label: directory.label,
    scope: workspaceScope(workspaceId, directory.sourceId, workspace),
  }));

/** Files over every directory of the workspace, one root each (#174). The
 *  route's `sourceId` is the opened file's root and its `file` a path in that
 *  root's directory; with no file open it is the first root. */
function mountFilesPane(body, { canonical, workspace, callRpc, cacheScope }) {
  const roots = filesRoots(workspace, canonical.workspaceId);
  const openAt = canonical.file ? { rootId: canonical.sourceId, path: canonical.file, line: canonical.line || null } : null;
  return renderFilesTab(body, {
    roots,
    layoutEntityId: workspaceLayoutCacheId(canonical.workspaceId),
    callRpc,
    cacheScope,
    openAt,
    onFileOpen: (path, rootId) => markRoute({ ...canonical, sourceId: path ? rootId : roots[0].id, file: path }),
    viewingContext: App.viewingContext,
  });
}

/** The URL names the commit the standing directory's surface has selected. */
const markCommit = (commit) => {
  const { commit: _commit, ...route } = App.route;
  markRoute(commit ? { ...route, commit } : route);
};

/** Changes over every directory of the workspace, a tab each (#174). A tab
 *  moves within the surface: its directory is a place in the URL, and the
 *  commit named there is the one that directory's surface has selected — one
 *  named in another directory names nothing in this one. */
function mountChangesPane(body, { canonical, workspace, callRpc, cacheScope, agentSelection, onSelectDirectory }) {
  const commits = new Map([[canonical.sourceId, canonical.commit || null]]);
  const changes = mountWorkspaceChanges(body, {
    directories: workspaceDirectoryModel(workspace),
    current: canonical.sourceId,
    viewingContext: App.viewingContext,
    onSelectDirectory: (sourceId) => onSelectDirectory(sourceId, commits.get(sourceId) || null),
    git: (sourceId, viewingContext) => ({
      scope: workspaceScope(canonical.workspaceId, sourceId, workspace),
      callRpc,
      cacheScope,
      agentSelection,
      projectId: canonical.projectId,
      navigate: { openFile: ({ path, line }) => go({ ...App.route, sourceId, tab: "files", file: path, line }) },
      viewingContext,
      requestedCommit: commits.get(sourceId) || null,
      onCommitSelection: (commit) => {
        commits.set(sourceId, commit);
        if (App.route.sourceId === sourceId) markCommit(commit);
      },
    }),
  });
  return { ...changes, workspaceMoved: (next) => changes.workspaceMoved(workspaceDirectoryModel(next)) };
}

function mountCheckoutPane(body, options) {
  return options.canonical.tab === "files" ? mountFilesPane(body, options) : mountChangesPane(body, options);
}

function paintGitInitialization(rail, state, sourceId, directory) {
  const git = gitInitOf(state, sourceId);
  git.painted = true;
  if (directory.is_git !== false && git.sourceGit == null) {
    probeSourceGit(state, sourceId);
    return;
  }
  const canInitialize = directory.is_git === false || git.workspaceNeedsReconciliation || git.sourceGit === false || git.sourceNeedsReconciliation;
  git.needsHost = canInitialize;
  let initHost = rail.querySelector(".workspace-init-host");
  if (!canInitialize) {
    state.gitInitialization.get(sourceId)?.controller.dispose();
    state.gitInitialization.delete(sourceId);
    initHost?.remove();
    return;
  }
  initHost = initHost || keepGitInitializationHost(rail, state, sourceId);
  initHost.querySelector("[data-init-git]").textContent = gitInitializationLabel(git, directory);
}

/** Reuse the controller when the commit rail repaints; its modal and pending
 * operation belong to the directory, not to a disposable rail element. */
function keepGitInitializationHost(rail, state, sourceId) {
  const held = state.gitInitialization.get(sourceId);
  if (held) {
    rail.appendChild(held.host);
    return held.host;
  }
  const host = document.createElement("div");
  host.className = "workspace-init-host";
  rail.appendChild(host);
  const controller = mountWorkspaceGitInitialization({
    host, workspaceId: state.route.workspaceId, sourceId, callRpc: state.callRpc,
    cacheScope: state.context.cacheScope,
    isActive: () => workspaceSourceIsActive(state, sourceId),
    holds: () => workspaceHoldsSource(state, sourceId),
    onUpdate: async (answer) => {
      if (answer.workspace) await writeWorkspaceResult(state, answer.workspace);
      await writeGitResult(state, sourceId, answer);
      const address = gitOptionsAddress(state, sourceId);
      return address ? (await readCached(address))?.value : null;
    },
  });
  state.gitInitialization.set(sourceId, { host, controller });
  return host;
}

function gitInitializationLabel(git, directory) {
  if (git.workspaceNeedsReconciliation) return "Finish Git initialization…";
  return directory.is_git === false ? "Initialize Git…" : "Initialize original source…";
}

/** Still this view's to write to: not disposed, and its machine is still the
 *  one the route is about and still able to answer. */
const ownsWorkspace = (state) => !state.disposed && state.context.active() && state.context === routeContext(App.route);

function workspaceSourceIsActive(state, sourceId) {
  if (!ownsWorkspace(state)) return false;
  if (App.route.name !== "workspace" || App.route.workspaceId !== state.route.workspaceId || App.route.sourceId !== sourceId) return false;
  return (state.workspace?.directories || []).some((directory) => directoryId(directory) === sourceId);
}

/** Still this view's, and a directory its workspace still has — whether or not
 *  it is the one standing. */
const workspaceHoldsSource = (state, sourceId) =>
  ownsWorkspace(state) && App.route.workspaceId === state.route.workspaceId &&
  (state.workspace?.directories || []).some((directory) => directoryId(directory) === sourceId);

async function probeSourceGit(state, sourceId) {
  const git = gitInitOf(state, sourceId);
  if (git.probePending) return;
  git.probePending = true;
  try {
    await readGitOptions(state, sourceId);
    const revision = state.gitOptionsRevision.get(sourceId) || 0;
    const options = await state.callRpc("workspace.git_init_options", { workspace_id: state.route.workspaceId, source_id: sourceId });
    if (!ownsWorkspace(state)) return;
    if ((state.gitOptionsRevision.get(sourceId) || 0) !== revision) return;
    const address = gitOptionsAddress(state, sourceId);
    if (address) await writeCached(address, options);
  } catch {
    // A failed probe adds no news; the cached source answer remains on screen.
  } finally {
    git.probePending = false;
  }
}

/** Where the git-initialization offer hangs on Changes, in the directory
 *  surface showing: the surface itself for a directory with no git
 *  (views/workspaceChanges.js), the commit rail for one whose original source
 *  still has none. It is rebuilt from scratch whenever the pane repaints.
 *  Files, a tree over every directory, carries none. */
const SHOWN_SURFACE = ".workspace-changes-surface:not([hidden])";
const offerHost = (body) => body.querySelector(`${SHOWN_SURFACE} .workspace-gitinit-offer, ${SHOWN_SURFACE} .crail-host`);

/** Hang a failed workspace's Retry and the git-initialization offer where
 *  Changes keeps them, for the directory standing. */
function gitOfferPainter(body, state) {
  return () => {
    const directory = selectedDirectory(state.workspace, state.sourceId);
    if (!directory) return false;
    const list = offerHost(body);
    if (!list) return true;
    paintWorkspaceAction(list, state);
    paintGitInitialization(list, state, state.sourceId, directory);
    return true;
  };
}

/** The offer lives inside the pane, and the pane rewrites itself — on a poll,
 *  on a ref checkout. Watch for the column coming back without it and put it
 *  there again; a directory that has nothing to initialize wants nothing put
 *  back, so it never repaints on its own DOM. The column appearing for a
 *  directory not painted yet is painted, once. */
function observeTabs(body, state, paintTabs, dispose) {
  const observer = new MutationObserver(() => {
    const list = offerHost(body);
    const git = gitInitOf(state, state.sourceId);
    const lost = (git.needsHost && !list?.querySelector(".workspace-init-host")) || (state.workspaceAction && !list?.querySelector(".workspace-action-host"));
    if (list && (!git.painted || lost)) paintTabs();
  });
  paintTabs();
  observer.observe(body, { childList: true, subtree: true });
  return () => {
    observer.disconnect();
    dispose();
  };
}

/** Where the surface stands for a route over this workspace: the directory.
 *  Files with no file open stands on the first root, since its tree is every
 *  directory's; any directory, git or not, stands on Changes, which offers to
 *  initialize one that has no git (#174). */
function canonicalRoute(route, workspace) {
  const directory = selectedDirectory(workspace, route.tab === "files" && !route.file ? null : route.sourceId);
  return { ...route, sourceId: directoryId(directory) };
}

/** A directory tab moved Changes onto another directory within the surface: the
 *  URL names it, and its offer is hung. */
function standOnDirectory(state, sourceId, commit) {
  const { commit: _commit, ...route } = App.route;
  state.route = { ...route, sourceId, tab: "changes", ...(commit && { commit }) };
  state.sourceId = sourceId;
  markRoute(state.route);
  state.gitInitialization.forEach((entry, id) => entry.controller.setVisible(id === sourceId));
  state.paintTabs?.();
}

function refreshWorkspacePane(state, workspace) {
  const canonical = canonicalRoute(state.route, workspace);
  const { sourceId } = canonical;
  if (!sourceId) return null;
  state.sourceId = sourceId;
  const body = $("#tabbody");
  const previousGuard = state.pane?.canLeave;
  if (App.routeLeaveGuard === previousGuard) App.routeLeaveGuard = null;
  state.pane?.dispose?.();
  state.gitInitialization.forEach(({ controller }) => controller.dispose());
  state.gitInitialization.clear();
  body.innerHTML = '<div class="empty">loading…</div>';
  state.pane = mountDirectoryPane(body, {
    canonical,
    workspace: state.workspace,
    callRpc: state.callRpc,
    cacheScope: state.context.cacheScope,
    agentSelection: state.selection,
    context: state.context,
    onSelectDirectory: (next, commit) => standOnDirectory(state, next, commit),
    // Read at every use, never captured: a workspace gains and loses agents
    // while the tab stands there, and the tasks follow them.
    feed: () => deviceFeedNow(state.route.deviceId),
  });
  App.routeLeaveGuard = state.pane?.canLeave || null;
  return { canonical, body };
}

function mountWorkspace(workspace, state) {
  if (state.disposed) return;
  const { route, callRpc } = state;
  state.workspace = workspace;
  installWorkspaceAction(state, workspace);
  const canonical = canonicalRoute(route, workspace);
  const { sourceId } = canonical;
  if (!sourceId) {
    $("#tabbody").innerHTML = errorHtml("This workspace has no source directories.");
    return;
  }

  // The tasks tab is not about a directory, so none is written into its URL —
  // the sourceId on the canonical route is only there for the console's scope.
  if (!onTasksTab(route) && (route.sourceId !== sourceId || route.tab !== canonical.tab)) markRoute(canonical);

  state.refreshPane = (nextWorkspace) => refreshWorkspacePane(state, nextWorkspace);
  const mounted = state.refreshPane(workspace);
  if (state.disposed) {
    state.pane?.dispose?.();
    return;
  }

  state.rail.paint(mounted.canonical.tab);
  state.paintTabs = gitOfferPainter(mounted.body, state);
  App.viewDispose = observeTabs(mounted.body, state, state.paintTabs, App.viewDispose);
}

export async function renderWorkspace() {
  const root = $("#root");
  const route = App.route;
  const context = surfaceContext(route);
  root.className = "surface";
  // The surface paints what the records hold of this machine whether or not it
  // can answer, and a workspace they do not name yet is waited for exactly as
  // it is while the machine answers (standOnWorkspace). Only a machine nothing
  // here has ever held has nothing to paint: the notice names it, waits for it,
  // and hands the link back when it lands.
  if (!context) {
    mountDeviceNotice(root, route.deviceId);
    return;
  }
  const state = { selection: shellSelection(), route, context, callRpc: context.rpc, disposed: false, pane: null, workspaceAction: null, refreshPane: null, workspace: null, sourceId: null, gitInit: new Map(), paintTabs: null, rail: null, gitInitialization: new Map(), unwatchFeed: null, workspaceRead: null, gitOptionsRead: null, gitOptionsRevision: new Map(), retryRefreshPending: false };
  const unwatchWorkspace = subscribeCache(workspaceListAddress(state), () => {
    state.workspaceRead = readWorkspaceResult(state);
  });
  const unwatchGitOptions = subscribeCache({ deviceId: route.deviceId, entityId: route.workspaceId, kind: "git-init-options" }, (address) => {
    if (!address.sub || !state.workspace) return;
    state.gitOptionsRevision.set(address.sub, (state.gitOptionsRevision.get(address.sub) || 0) + 1);
    state.gitOptionsRead = readGitOptions(state, address.sub);
  });
  root.innerHTML = `<div id="tabbody" class="flush"><div class="empty">loading…</div></div>`;
  // The rail is the workspace's, not a directory's: it stands on the route
  // alone, before the record naming the workspace has landed.
  state.rail = mountWorkspaceRail($("#dir-rail"), {
    route,
    feed: () => deviceFeedNow(route.deviceId),
    workspace: () => state.workspace,
    onSelect: (tab) => go(tab === TASKS_TAB ? workspaceTasksPlace(App.route) : { ...App.route, tab }),
    navigate: go,
  });
  state.rail.paint(route.tab);
  // While the machine cannot answer, what the records hold stays on screen and
  // the strip says whose state that is — but only once there is something to be
  // whose: until the workspace lands this frame says "loading…", and nothing on
  // it came from that machine at all.
  const deviceStrip = mountDeviceStrip(root, context, { hasContent: () => Boolean(state.workspace) });
  App.viewDispose = () => {
    state.disposed = true;
    // The view stops hearing the feed rather than trusting the shell to clear
    // the slot it put it in.
    state.unwatchFeed?.();
    state.unwatchFeed = null;
    unwatchWorkspace();
    unwatchGitOptions();
    // The rail is the shell's column, lent to whichever surface is standing on
    // it: leaving hands it back empty rather than leaving this workspace's
    // faces up over the next view.
    state.rail.dispose();
    deviceStrip();
    if (App.routeLeaveGuard === state.pane?.canLeave) App.routeLeaveGuard = null;
    state.pane?.dispose?.();
    state.gitInitialization.forEach(({ controller }) => controller.dispose());
  };
  // The workspace is a record: the pass writes the machine's checkouts and a
  // board push rewrites them, so the surface stands on the list rather than
  // asking for the one row again. Its conversation is the rail's, and the rail
  // pages that out of the cache for itself.
  await standOnWorkspace(state);
}

/** This machine's checkout under the routed id, out of the feed's slice of the
 *  workspace records, or null while nothing there names it. */
const workspaceNow = (deviceId, workspaceId) =>
  (deviceFeedNow(deviceId)?.workspaces || []).find((candidate) => candidate.id === workspaceId) || null;

/**
 * Stand the surface up on the machine's checkout list, and keep listening
 * until that list names this workspace.
 *
 * A list the workspace is not in yet is not a wrong link. Both things that
 * write it — a pass and a board push — are round trips, and the commonest way
 * into this surface is the one that outruns them: Create workspace answers,
 * navigates here on the spot, and the record naming what it just made lands
 * after the frame. The same holds for a deep link opened against a cold cache,
 * where the first pass is still running. So the list is subscribed to, and the
 * delivery that carries the workspace is what mounts it.
 *
 * The name is only wrong once a pass this tab asked for has answered and the
 * records still do not carry it. Where no pass ran — another tab holds the
 * sync lock, or one was already running — nothing has been established, so the
 * frame keeps waiting and the subscription mounts it when that pass writes.
 * Either way the subscription stays up: a later push can still name it.
 */
async function standOnWorkspace(state) {
  const take = () => {
    if (state.disposed) return;
    // The Tasks count on the rail reads the same roster.
    state.rail.feedMoved();
    if (state.workspace) {
      // The workspace Tasks pane derives its scope from the cached feed's
      // roster. Keep the cache announcement wired after mount so a later row
      // write can re-scope the already cached task catalogue.
      state.pane?.feedMoved?.();
      return;
    }
    const workspace = workspaceNow(state.route.deviceId, state.route.workspaceId);
    if (!workspace) return;
    mountWorkspace(workspace, state);
  };
  state.unwatchFeed = subscribeFeed(take);
  if (state.workspace || state.disposed) return;
  if (await passAnsweredWithoutIt(state)) sayUnknownWorkspace(state);
}

/** Whether a pass this tab asked for has answered and the machine's checkouts
 *  still do not name this workspace — the one thing that makes the link wrong
 *  rather than early.
 *
 *  Off the records rather than off the feed: the pass has written, and the
 *  announcement behind that write reaches the feed a turn later. Reading the
 *  disk is what makes "not there" an answer instead of a race with it. */
async function passAnsweredWithoutIt(state) {
  const { route } = state;
  const [passed] = await refreshFeed(route.deviceId);
  if (state.workspace || state.disposed || !passed) return false;
  const workspaces = (await cachedFeedView(route.deviceId)).workspaces || [];
  if (state.workspace || state.disposed) return false;
  return !workspaces.some((candidate) => candidate.id === route.workspaceId);
}

function sayUnknownWorkspace(state) {
  const body = $("#tabbody");
  if (body) body.innerHTML = errorHtml(`unknown workspace_id: ${state.route.workspaceId}`);
}
