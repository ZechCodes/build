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
import { directoryId, refLabel, selectedDirectory, workspaceScope } from "../core/workspaceModel.js";
import "../styles/surfaces.css";

const TABS = [
  { id: "changes", label: "Changes" },
  { id: "files", label: "Files" },
];

function mountChanges(body, { scope, callRpc, projectId, navigate, viewingContext }) {
  body.innerHTML = `<div class="workspace-gitpane"></div>`;
  const refbar = document.createElement("div");
  refbar.className = "workspace-refbar";
  refbar.innerHTML = `
    <label for="workspace-ref">Branch or tag</label>
    <select id="workspace-ref" aria-label="Branch or tag" disabled><option>Loading refs…</option></select>
    <span class="workspace-refstate"></span><span class="error workspace-referror" role="status"></span>`;
  const select = refbar.querySelector("#workspace-ref");
  const state = refbar.querySelector(".workspace-refstate");
  const errorHost = refbar.querySelector(".workspace-referror");
  const gitHost = body.querySelector(".workspace-gitpane");
  let gitPane = mountGitPane(gitHost, { scope, callRpc, projectId, navigate, viewingContext });
  let disposed = false;
  let currentRef = "";
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
  const loadRefs = async () => {
    const answer = await callRpc("git.refs", scope);
    if (disposed) return;
    const refs = answer.refs || [];
    currentRef = answer.current?.full_ref || refs.find((entry) => entry.current)?.full_ref || "";
    const detached = answer.current?.kind === "detached" && !refs.some((entry) => entry.full_ref === currentRef)
      ? `<option value="" disabled selected>${esc(refLabel(answer.current))}</option>`
      : "";
    select.innerHTML = detached + refs.map((entry) =>
      `<option value="${esc(entry.full_ref)}"${entry.full_ref === currentRef ? " selected" : ""}>${esc(entry.name)}${entry.kind === "tag" ? " (tag)" : ""}</option>`,
    ).join("");
    select.disabled = refs.length === 0;
    state.textContent = refLabel(answer.current);
  };
  loadRefs().catch((error) => {
    select.innerHTML = `<option>Refs unavailable</option>`;
    errorHost.textContent = error.message || String(error);
  });
  select.onchange = async () => {
    const requested = select.value;
    select.disabled = true;
    errorHost.textContent = "";
    try {
      await callRpc("git.checkout_ref", { ...scope, full_ref: requested });
      if (disposed) return;
      await loadRefs();
      if (disposed) return;
      gitPane.dispose();
      gitPane = mountGitPane(gitHost, { scope, callRpc, projectId, navigate, viewingContext });
    } catch (error) {
      select.value = currentRef;
      select.disabled = false;
      errorHost.textContent = error.message || String(error);
    }
  };
  return { dispose: () => {
    disposed = true;
    attachObserver.disconnect();
    gitPane.dispose();
  } };
}

function errorHtml(message) {
  return `<div class="empty"><h2>Workspace unavailable</h2><p>${esc(message)}</p></div>`;
}

function workspaceAction(workspace) {
  return workspace?.status === "failed" ? { label: "Retry", method: "workspace.retry" } : { label: "Finish", method: "workspace.finish" };
}

function finishOutcome(answer) {
  if (answer.complete) return { message: "Finished. Workspace files were kept.", failed: false };
  const failures = (answer.repositories || []).filter((repository) => !repository.pushed);
  const detail = failures.map((repository) => `${repository.directory_id}: ${repository.reason || "not pushed"}`).join(" · ");
  return { message: detail || "Workspace has incomplete or missing directories and cannot be finished.", failed: true };
}

function applyRetry(answer, state, previous) {
  const selectedNeedsRefresh = selectedDirectory(previous, state.route.sourceId)?.status !== "ready";
  const current = answer.workspace || answer;
  if (selectedNeedsRefresh) state.refreshPane?.(current);
  return current;
}

function installWorkspaceAction(state, workspace) {
  let current = workspace;
  let pending = false;
  let message = "";
  let failed = false;
  const render = (host) => {
    const action = workspaceAction(current);
    host.innerHTML = `<span class="workspace-action-status ${failed ? "error" : ""}" role="status">${esc(message)}</span>
      <button class="btn mini" type="button" data-workspace-action${pending ? " disabled" : ""}>${esc(action.label)}</button>`;
    host.querySelector("[data-workspace-action]").onclick = async () => {
      pending = true;
      message = "";
      failed = false;
      render(host);
      try {
        const answer = await state.callRpc(action.method, { workspace_id: state.route.workspaceId });
        if (state.disposed) return;
        if (action.method === "workspace.finish") {
          ({ message, failed } = finishOutcome(answer));
        } else {
          current = applyRetry(answer, state, current);
          message = "Workspace ready.";
        }
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

function mountDirectoryPane(body, { directory, canonical, scope, callRpc }) {
  const navigate = { openFile: ({ path, line }) => go({ ...canonical, tab: "files", file: path, line }) };
  if (canonical.tab !== "files") {
    return mountChanges(body, {
      scope,
      callRpc,
      projectId: canonical.projectId,
      navigate,
      viewingContext: App.viewingContext,
    });
  }
  const openAt = canonical.file ? { path: canonical.file, line: canonical.line || null } : null;
  return renderFilesTab(body, {
    scope,
    callRpc,
    openAt,
    onFileOpen: (path) => markRoute({ ...canonical, file: path }),
    viewingContext: App.viewingContext,
  });
}

function directoryTabsPainter(body, directory, canonical) {
  return () => {
    const rail = body.querySelector(".crail-host, .ftree");
    if (!rail) return false;
    let host = rail.querySelector(".railtabs");
    if (!host) {
      host = document.createElement("div");
      host.className = "railtabs";
      rail.appendChild(host);
    }
    const tabs = directory.is_git === false ? TABS.filter((entry) => entry.id === "files") : TABS;
    host.innerHTML = tabShellHtml({ tabs, active: canonical.tab });
    host.querySelectorAll("[data-tab]").forEach((control) => {
      control.onclick = () => go({ ...canonical, tab: control.dataset.tab });
    });
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
  const tab = directory.is_git === false ? "files" : state.route.tab || "changes";
  const canonical = { ...state.route, sourceId, tab };
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
  });
  App.routeLeaveGuard = state.pane?.canLeave || null;
  return { directory, canonical, body };
}

function mountWorkspaceAgentRail(workspace, route, sourceId) {
  return mountAgentRail($("#agent-rail"), {
    kind: "workspace",
    workspaceId: route.workspaceId,
    sourceId,
    projectId: workspace.project_id || route.projectId,
    selection: createAgentSelection(),
  });
}

function mountWorkspace(workspace, state) {
  if (state.disposed) return;
  const { route, callRpc } = state;
  installWorkspaceAction(state, workspace);
  const directory = selectedDirectory(workspace, route.sourceId);
  const sourceId = directoryId(directory);
  state.agentRail = mountWorkspaceAgentRail(workspace, route, sourceId);
  if (!sourceId) {
    $("#tabbody").innerHTML = errorHtml("This workspace has no source directories.");
    state.consolePanel = mountConsole($("#console-region"), { kind: "workspace", workspaceId: route.workspaceId });
    return;
  }

  const tab = directory.is_git === false ? "files" : route.tab || "changes";
  const canonical = { ...route, sourceId, tab };
  if (route.sourceId !== sourceId || route.tab !== tab) markRoute(canonical);

  state.refreshPane = (nextWorkspace) => refreshWorkspacePane(state, nextWorkspace);
  const mounted = state.refreshPane(workspace);
  if (state.disposed) {
    state.pane?.dispose?.();
    return;
  }

  App.viewDispose = observeTabs(mounted.body, directoryTabsPainter(mounted.body, mounted.directory, mounted.canonical), App.viewDispose);
  state.consolePanel = mountConsole($("#console-region"), { kind: "workspace", workspaceId: route.workspaceId });
}

export async function renderWorkspace() {
  const root = $("#root");
  const state = { route: App.route, callRpc: App.call, disposed: false, pane: null, consolePanel: null, agentRail: null, toolbarAction: null, refreshPane: null };
  root.className = "surface";
  root.innerHTML = `<div id="tabbody" class="flush"><div class="empty">loading…</div></div>`;
  App.viewDispose = () => {
    state.disposed = true;
    if (App.routeLeaveGuard === state.pane?.canLeave) App.routeLeaveGuard = null;
    state.pane?.dispose?.();
    state.consolePanel?.dispose?.();
    state.agentRail?.dispose?.();
    clearToolbarVerb(state.toolbarAction);
  };
  try {
    const response = await state.callRpc("workspace.get", { workspace_id: state.route.workspaceId });
    mountWorkspace(response.workspace || response, state);
  } catch (error) {
    if (!state.disposed) $("#tabbody").innerHTML = errorHtml(error.message || String(error));
  }
}
