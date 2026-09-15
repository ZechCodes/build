// A workspace is a durable root containing one or more registered source
// directories. Directory selection scopes Files and Changes; the console stays
// scoped to the workspace, so navigating between directories or refs never
// replaces its server sessions.

import { $ } from "../dom.js";
import { App, go, markRoute } from "../app.js";
import { esc } from "../core/text.js";
import { tabShellHtml } from "../core/tabshell.js";
import { mountGitPane } from "../core/gitPane.js";
import { mountConsole } from "../core/console.js";
import { mountAgentRail } from "../core/agentRail.js";
import { createAgentSelection } from "../core/agentSelection.js";
import { clearToolbarVerb, setToolbarVerb } from "../core/toolbar.js";
import { renderFilesTab } from "./files.js";
import { directoryId, directoryTab, selectedDirectory, workspaceScope } from "../core/workspaceModel.js";
import { mountWorkspaceRefPicker } from "../core/workspaceRefPicker.js";
import { mountWorkspaceGitInitialization } from "../core/workspaceGitInitialization.js";
import { canAnswer, routeContext } from "../core/deviceContexts.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { SMALLEST_THREAD_PAGE } from "../core/thread.js";
import "../styles/surfaces.css";

const TABS = [
  { id: "changes", label: "Changes" },
  { id: "files", label: "Files" },
];

function mountChanges(body, { scope, callRpc, cacheScope, projectId, navigate, viewingContext }) {
  body.innerHTML = `<div class="workspace-gitpane"></div>`;
  const refbar = document.createElement("div");
  refbar.className = "workspace-refbar";
  const gitHost = body.querySelector(".workspace-gitpane");
  let gitPane = mountGitPane(gitHost, { scope, callRpc, cacheScope, projectId, navigate, viewingContext });
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
      gitPane = mountGitPane(gitHost, { scope, callRpc, projectId, navigate, viewingContext });
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

function mountDirectoryPane(body, { directory, canonical, scope, callRpc, cacheScope }) {
  const navigate = { openFile: ({ path, line }) => go({ ...canonical, tab: "files", file: path, line }) };
  if (canonical.tab !== "files") {
    return mountChanges(body, {
      scope,
      callRpc,
      cacheScope,
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

function directoryTabsPainter(body, state, sourceId) {
  return () => {
    const directory = selectedDirectory(state.workspace, sourceId);
    if (!directory) return false;
    const rail = body.querySelector(".crail-host, .ftree");
    if (!rail) return false;
    let host = rail.querySelector(".railtabs");
    if (!host) {
      host = document.createElement("div");
      host.className = "railtabs";
      rail.appendChild(host);
    }
    const tabs = directory.is_git === false ? TABS.filter((entry) => entry.id === "files") : TABS;
    host.innerHTML = tabShellHtml({ tabs, active: App.route.tab });
    host.querySelectorAll("[data-tab]").forEach((control) => {
      control.onclick = () => go({ ...App.route, tab: control.dataset.tab });
    });
    paintGitInitialization(rail, state, sourceId, directory);
    return true;
  };
}

function observeTabs(body, paintTabs, dispose) {
  const observer = new MutationObserver(() => {
    const rail = body.querySelector(".crail-host, .ftree");
    if (rail && !rail.querySelector(".railtabs")) paintTabs();
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
    scope: workspaceScope(state.route.workspaceId, sourceId),
    callRpc: state.callRpc,
    cacheScope: state.context.cacheScope,
  });
  App.routeLeaveGuard = state.pane?.canLeave || null;
  return { directory, canonical, body };
}

/** The console is the workspace's, not a directory's: moving between
 *  directories or refs never replaces the sessions it is holding. */
const mountWorkspaceConsole = (state) =>
  mountConsole($("#console-region"), {
    kind: "workspace",
    workspaceId: state.route.workspaceId,
    deviceId: state.context.deviceId,
  });

function mountWorkspaceAgentRail(workspace, state, sourceId) {
  const { route, context } = state;
  return mountAgentRail($("#agent-rail"), {
    kind: "workspace",
    workspaceId: route.workspaceId,
    sourceId,
    projectId: workspace.project_id || route.projectId,
    deviceId: context.deviceId,
    callRpc: context.rpc,
    cacheScope: context.cacheScope,
    chatRepository: context.chatRepository,
    selection: createAgentSelection(),
  });
}

function mountWorkspace(workspace, state) {
  if (state.disposed) return;
  const { route, callRpc } = state;
  state.workspace = workspace;
  installWorkspaceAction(state, workspace);
  const directory = selectedDirectory(workspace, route.sourceId);
  const sourceId = directoryId(directory);
  state.agentRail = mountWorkspaceAgentRail(workspace, state, sourceId);
  if (!sourceId) {
    $("#tabbody").innerHTML = errorHtml("This workspace has no source directories.");
    state.consolePanel = mountWorkspaceConsole(state);
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
  App.viewDispose = observeTabs(mounted.body, state.paintTabs, App.viewDispose);
  state.consolePanel = mountWorkspaceConsole(state);
}

export async function renderWorkspace() {
  const root = $("#root");
  const route = App.route;
  const context = routeContext(route);
  root.className = "surface";
  // A machine that cannot answer — never opened here, or gone since — has
  // nothing under this link to read or write, so the surface names it rather
  // than standing a frame up over calls that can only be refused.
  if (!canAnswer(context)) {
    mountDeviceNotice(root, route.deviceId);
    return;
  }
  const state = { route, context, callRpc: context.rpc, disposed: false, pane: null, consolePanel: null, agentRail: null, toolbarAction: null, refreshPane: null, workspace: null, workspaceNeedsReconciliation: false, sourceGit: null, sourceNeedsReconciliation: false, sourceProbePending: false, paintTabs: null, gitInitialization: [] };
  root.innerHTML = `<div id="tabbody" class="flush"><div class="empty">loading…</div></div>`;
  // This machine answers now. If it goes while the workspace is open, what was
  // read stays on screen and the strip says whose state that is — but only once
  // there is something to be whose: until the workspace lands this frame says
  // "loading…", and nothing on it came from that machine at all.
  const deviceStrip = mountDeviceStrip(root, context, { hasContent: () => Boolean(state.workspace) });
  App.viewDispose = () => {
    state.disposed = true;
    deviceStrip();
    if (App.routeLeaveGuard === state.pane?.canLeave) App.routeLeaveGuard = null;
    state.pane?.dispose?.();
    state.consolePanel?.dispose?.();
    state.agentRail?.dispose?.();
    state.gitInitialization.forEach((controller) => controller.dispose());
    clearToolbarVerb(state.toolbarAction);
  };
  try {
    // This read mounts workspace metadata; the agent rail opens and pages its
    // own selected conversation. Ask for the smallest valid thread window so
    // an old conversation never bloats the surface's initial response.
    const response = await state.callRpc("workspace.get", {
      workspace_id: state.route.workspaceId,
      ...SMALLEST_THREAD_PAGE,
    });
    mountWorkspace(response.workspace || response, state);
  } catch (error) {
    if (!state.disposed) $("#tabbody").innerHTML = errorHtml(error.message || String(error));
  }
}
