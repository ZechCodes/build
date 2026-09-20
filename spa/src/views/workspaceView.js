// A workspace is a durable root containing one or more registered source
// directories. Directory selection scopes Files and Changes; the console stays
// scoped to the workspace, so navigating between directories or refs never
// replaces its server sessions.

import { $ } from "../dom.js";
import { App, go, markRoute } from "../app.js";
import { esc } from "../core/text.js";
import { DIRECTORY_TABS, paintDirectoryRail } from "../core/directoryRail.js";
import { mountGitPane } from "../core/gitPane.js";
import { shellSelection } from "../core/shell.js";
import { clearToolbarVerb, setToolbarVerb } from "../core/toolbar.js";
import { renderFilesTab } from "./files.js";
import { directoryId, directoryTab, selectedDirectory, workspaceScope } from "../core/workspaceModel.js";
import { mountWorkspaceRefPicker } from "../core/workspaceRefPicker.js";
import { mountWorkspaceGitInitialization } from "../core/workspaceGitInitialization.js";
import { canAnswer, knownDeviceContext, routeContext } from "../core/deviceContexts.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { deviceFeedNow } from "../core/feedRows.js";
import { cachedFeedView } from "../core/cachedRows.js";
import { refreshFeed, subscribeFeed } from "../core/taskFeed.js";
import "../styles/surfaces.css";

function mountChanges(body, { scope, callRpc, cacheScope, projectId, navigate, viewingContext, agentSelection }) {
  body.innerHTML = `<div class="workspace-gitpane"></div>`;
  const refbar = document.createElement("div");
  refbar.className = "workspace-refbar";
  const gitHost = body.querySelector(".workspace-gitpane");
  let gitPane = mountGitPane(gitHost, { scope, callRpc, cacheScope, projectId, navigate, viewingContext, agentSelection });
  let disposed = false;
  const attachRefbar = () => {
    const rail = gitHost.querySelector(".crail-host");
    if (!rail) return false;
    if (refbar.parentElement === rail && rail.firstElementChild === refbar) return true;
    rail.prepend(refbar);
    return true;
  };
  const attachObserver = new MutationObserver(attachRefbar);
  attachRefbar();
  attachObserver.observe(gitHost, { childList: true, subtree: true });
  const refPicker = mountWorkspaceRefPicker(refbar, { scope, callRpc, onCheckout: async () => {
      if (disposed) return;
      gitPane.dispose();
      gitPane = mountGitPane(gitHost, { scope, callRpc, cacheScope, projectId, navigate, viewingContext, agentSelection });
    } });
  return { dispose: () => {
    disposed = true;
    attachObserver.disconnect();
    refPicker.dispose();
    gitPane.dispose();
  } };
}

function errorHtml(message) {
  return `<div class="empty"><h2>Workspace unavailable</h2><p>${esc(message)}</p></div>`;
}

function applyRetry(answer, state, previous) {
  const selectedNeedsRefresh = selectedDirectory(previous, state.route.sourceId)?.status !== "ready";
  const current = answer.workspace || answer;
  if (selectedNeedsRefresh) state.refreshPane?.(current);
  return current;
}

function installWorkspaceAction(state, workspace) {
  if (workspace?.status !== "failed") return;
  let current = workspace;
  let pending = false;
  let message = "";
  let failed = false;
  const render = (host) => {
    if (current?.status !== "failed") {
      host.innerHTML = `<span class="workspace-action-status" role="status">${esc(message)}</span>`;
      return;
    }
    host.innerHTML = `<span class="workspace-action-status ${failed ? "error" : ""}" role="status">${esc(message)}</span>
      <button class="btn mini" type="button" data-workspace-action${pending ? " disabled" : ""}>Retry</button>`;
    host.querySelector("[data-workspace-action]").onclick = async () => {
      pending = true;
      message = "";
      failed = false;
      render(host);
      try {
        const answer = await state.callRpc("workspace.retry", { workspace_id: state.route.workspaceId });
        if (state.disposed) return;
        current = applyRetry(answer, state, current);
        message = "Workspace ready.";
      } catch (error) {
        if (state.disposed) return;
        failed = true;
        message = error.message || String(error);
      } finally {
        pending = false;
        if (!state.disposed) render(host);
      }
    };
  };
  state.toolbarAction = render;
  setToolbarVerb(render);
}

function mountDirectoryPane(body, { directory, canonical, scope, callRpc, cacheScope, agentSelection }) {
  const navigate = { openFile: ({ path, line }) => go({ ...canonical, tab: "files", file: path, line }) };
  if (canonical.tab !== "files") {
    return mountChanges(body, {
      scope,
      callRpc,
      cacheScope,
      agentSelection,
      projectId: canonical.projectId,
      navigate,
      viewingContext: App.viewingContext,
    });
  }
  const openAt = canonical.file ? { path: canonical.file, line: canonical.line || null } : null;
  return renderFilesTab(body, {
    scope,
    callRpc,
    cacheScope,
    openAt,
    onFileOpen: (path) => markRoute({ ...canonical, file: path }),
    viewingContext: App.viewingContext,
  });
}

function paintGitInitialization(rail, state, sourceId, directory) {
  if (directory.is_git !== false && state.sourceGit == null) {
    probeSourceGit(state, sourceId);
    return;
  }
  const canInitialize = directory.is_git === false || state.workspaceNeedsReconciliation || state.sourceGit === false || state.sourceNeedsReconciliation;
  state.needsInitHost = canInitialize;
  let initHost = rail.querySelector(".workspace-init-host");
  if (!canInitialize) {
    initHost?.remove();
    return;
  }
  if (!initHost) {
    initHost = document.createElement("div");
    initHost.className = "workspace-init-host";
    rail.appendChild(initHost);
    const controller = mountWorkspaceGitInitialization({
      host: initHost, workspaceId: state.route.workspaceId, sourceId, callRpc: state.callRpc,
      isActive: () => workspaceSourceIsActive(state, sourceId),
      onUpdate: (answer) => {
        if (answer.workspace) state.workspace = answer.workspace;
        if (answer.source && typeof answer.source.is_git === "boolean") state.sourceGit = answer.source.is_git;
        clearReconciledTargets(state, answer.results || answer.outcomes || []);
        state.paintTabs?.();
      },
    });
    state.gitInitialization.push(controller);
  }
  const initButton = initHost.querySelector("[data-init-git]");
  initButton.textContent = gitInitializationLabel(state, directory);
}

function gitInitializationLabel(state, directory) {
  if (state.workspaceNeedsReconciliation) return "Finish Git initialization…";
  return directory.is_git === false ? "Initialize Git…" : "Initialize original source…";
}

function clearReconciledTargets(state, results) {
  const completed = new Set(results.filter((result) => result.status !== "failed").map((result) => result.target));
  if (completed.has("workspace") || completed.has("both")) state.workspaceNeedsReconciliation = false;
  if (completed.has("source") || completed.has("both")) state.sourceNeedsReconciliation = false;
}

/** Still this view's to write to: not disposed, and its machine is still the
 *  one the route is about and still able to answer. */
const ownsWorkspace = (state) => !state.disposed && state.context.active() && state.context === routeContext(App.route);

function workspaceSourceIsActive(state, sourceId) {
  if (!ownsWorkspace(state)) return false;
  if (App.route.name !== "workspace" || App.route.workspaceId !== state.route.workspaceId || App.route.sourceId !== sourceId) return false;
  return (state.workspace?.directories || []).some((directory) => directoryId(directory) === sourceId);
}

async function probeSourceGit(state, sourceId) {
  if (state.sourceProbePending) return;
  state.sourceProbePending = true;
  try {
    const options = await state.callRpc("workspace.git_init_options", { workspace_id: state.route.workspaceId, source_id: sourceId });
    if (!ownsWorkspace(state)) return;
    state.sourceGit = options.source?.is_git !== false;
    state.sourceNeedsReconciliation = options.source?.needs_reconciliation === true;
    state.workspaceNeedsReconciliation = options.workspace?.needs_reconciliation === true;
    state.paintTabs?.();
  } catch {
    if (workspaceSourceIsActive(state, sourceId)) state.sourceGit = false;
  } finally {
    state.sourceProbePending = false;
  }
}

/** The pane's list column — the commit rail or the file tree — whichever tab is
 *  mounted. It is where the git-initialization offer hangs, and it is rebuilt
 *  from scratch whenever the pane repaints. */
const paneListColumn = (body) => body.querySelector(".crail-host, .ftree");

/** Paint the directory's two faces onto the SHELL's rail, and hang the
 *  git-initialization offer off the pane's list column. The rail is not in the
 *  pane: a pane remount leaves it standing, and a phone — where the list column
 *  is a drawer — never buries the switch inside one. */
function directoryTabsPainter(body, state, sourceId) {
  return () => {
    const directory = selectedDirectory(state.workspace, sourceId);
    if (!directory) return false;
    const tabs = directory.is_git === false ? DIRECTORY_TABS.filter((entry) => entry.id === "files") : DIRECTORY_TABS;
    paintDirectoryRail($("#dir-rail"), {
      tabs,
      active: App.route.tab,
      onSelect: (tab) => go({ ...App.route, tab }),
    });
    const list = paneListColumn(body);
    if (list) paintGitInitialization(list, state, sourceId, directory);
    return true;
  };
}

/** The offer lives inside the pane, and the pane rewrites itself — on a poll,
 *  on a ref checkout. Watch for the column coming back without it and put it
 *  there again; a directory that has nothing to initialize wants nothing put
 *  back, so it never repaints on its own DOM. */
function observeTabs(body, state, paintTabs, dispose) {
  const observer = new MutationObserver(() => {
    const list = paneListColumn(body);
    if (state.needsInitHost && list && !list.querySelector(".workspace-init-host")) paintTabs();
  });
  paintTabs();
  observer.observe(body, { childList: true, subtree: true });
  return () => {
    observer.disconnect();
    dispose();
  };
}

function refreshWorkspacePane(state, workspace) {
  const directory = selectedDirectory(workspace, state.route.sourceId);
  const sourceId = directoryId(directory);
  if (!sourceId) return null;
  const canonical = { ...state.route, sourceId, tab: directoryTab(directory, state.route.tab) };
  const body = $("#tabbody");
  const previousGuard = state.pane?.canLeave;
  if (App.routeLeaveGuard === previousGuard) App.routeLeaveGuard = null;
  state.pane?.dispose?.();
  body.innerHTML = '<div class="empty">loading…</div>';
  state.pane = mountDirectoryPane(body, {
    directory,
    canonical,
    scope: workspaceScope(state.route.workspaceId, sourceId, state.workspace),
    callRpc: state.callRpc,
    cacheScope: state.context.cacheScope,
    agentSelection: state.selection,
  });
  App.routeLeaveGuard = state.pane?.canLeave || null;
  return { directory, canonical, body };
}

function mountWorkspace(workspace, state) {
  if (state.disposed) return;
  const { route, callRpc } = state;
  state.workspace = workspace;
  installWorkspaceAction(state, workspace);
  const directory = selectedDirectory(workspace, route.sourceId);
  const sourceId = directoryId(directory);
  if (!sourceId) {
    $("#tabbody").innerHTML = errorHtml("This workspace has no source directories.");
    return;
  }

  const canonical = { ...route, sourceId, tab: directoryTab(directory, route.tab) };
  if (route.sourceId !== sourceId || route.tab !== canonical.tab) markRoute(canonical);

  state.refreshPane = (nextWorkspace) => refreshWorkspacePane(state, nextWorkspace);
  const mounted = state.refreshPane(workspace);
  if (state.disposed) {
    state.pane?.dispose?.();
    return;
  }

  state.paintTabs = directoryTabsPainter(mounted.body, state, sourceId);
  App.viewDispose = observeTabs(mounted.body, state, state.paintTabs, App.viewDispose);
}

export async function renderWorkspace() {
  const root = $("#root");
  const route = App.route;
  // The records may hold this workspace before its machine has a context at
  // all — a cold reload, the session not yet attempted. A session-less context
  // stands in (it answers nothing, and the session retargets it when it lands)
  // so the surface paints from disk rather than waiting on the wire.
  const held = workspaceNow(route.deviceId, route.workspaceId);
  const context = routeContext(route) || (held && route.deviceId ? knownDeviceContext(route.deviceId) : null);
  root.className = "surface";
  // A machine that cannot answer has nothing under this link to WRITE — but
  // what the records hold of it can still be read. A workspace the machine's
  // cached checkout list names paints from those records, with the strip
  // naming the machine over it; only a link to a machine never opened here, or
  // to a workspace nothing here has seen, stands the notice up instead.
  if (!canAnswer(context) && !(context && held)) {
    mountDeviceNotice(root, route.deviceId);
    return;
  }
  const state = { selection: shellSelection(), route, context, callRpc: context.rpc, disposed: false, pane: null, toolbarAction: null, refreshPane: null, workspace: null, workspaceNeedsReconciliation: false, sourceGit: null, sourceNeedsReconciliation: false, sourceProbePending: false, paintTabs: null, needsInitHost: false, gitInitialization: [], unwatchFeed: null };
  root.innerHTML = `<div id="tabbody" class="flush"><div class="empty">loading…</div></div>`;
  // This machine answers now. If it goes while the workspace is open, what was
  // read stays on screen and the strip says whose state that is — but only once
  // there is something to be whose: until the workspace lands this frame says
  // "loading…", and nothing on it came from that machine at all.
  const deviceStrip = mountDeviceStrip(root, context, { hasContent: () => Boolean(state.workspace) });
  App.viewDispose = () => {
    state.disposed = true;
    // The view stops hearing the feed rather than trusting the shell to clear
    // the slot it put it in.
    state.unwatchFeed?.();
    state.unwatchFeed = null;
    // The rail is the shell's column, lent to whichever surface is standing on
    // it: leaving hands it back empty rather than leaving this workspace's
    // faces up over the next view.
    $("#dir-rail").innerHTML = "";
    deviceStrip();
    if (App.routeLeaveGuard === state.pane?.canLeave) App.routeLeaveGuard = null;
    state.pane?.dispose?.();
    state.gitInitialization.forEach((controller) => controller.dispose());
    clearToolbarVerb(state.toolbarAction);
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
    if (state.disposed || state.workspace) return;
    const workspace = workspaceNow(state.route.deviceId, state.route.workspaceId);
    if (!workspace) return;
    stopWatchingFeed(state);
    mountWorkspace(workspace, state);
  };
  state.unwatchFeed = subscribeFeed(take);
  if (state.workspace || state.disposed) {
    // The replay mounted it inside `subscribeFeed`, before the handle above
    // existed to be let go of.
    stopWatchingFeed(state);
    return;
  }
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

function stopWatchingFeed(state) {
  state.unwatchFeed?.();
  state.unwatchFeed = null;
}
