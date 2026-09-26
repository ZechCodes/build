// A workspace is a durable root containing one or more registered source
// directories. Directory selection scopes Files and Changes; the console stays
// scoped to the workspace, so navigating between directories or refs never
// replaces its server sessions. The rail down the left edge is the workspace's
// navigation — Changes, Files, Issues, Settings (core/workspaceRail.js) — and
// stands on the route alone.

import { $ } from "../dom.js";
import { App, go, markRoute } from "../app.js";
import { esc } from "../core/text.js";
import { mountWorkspaceRail } from "../core/workspaceRail.js";
import { mountGitPane } from "../core/gitPane.js";
import { shellSelection } from "../core/shell.js";
import { clearToolbarVerb, setToolbarVerb } from "../core/toolbar.js";
import { renderFilesTab } from "./files.js";
import { directoryId, directoryTab, selectedDirectory, workspaceScope } from "../core/workspaceModel.js";
import { mountWorkspaceRefPicker } from "../core/workspaceRefPicker.js";
import { mountWorkspaceGitInitialization } from "../core/workspaceGitInitialization.js";
import { routeContext } from "../core/deviceContexts.js";
import { surfaceContext } from "../core/surfaceContext.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { deviceFeedNow } from "../core/feedRows.js";
import { cachedFeedView } from "../core/cachedRows.js";
import { mergeCached, readCached, subscribeCache, writeCached } from "../core/localCache.js";
import { upsertSessionRow } from "../core/sessionListCache.js";
import { refreshFeed, subscribeFeed } from "../core/taskFeed.js";
import { mountWorkspaceIssuesTab, workspaceIssuesPlace } from "../core/workspaceIssuesTab.js";
import { issueContextItem } from "../core/trackerViewingContext.js";
import "../styles/issues.css";
import "../styles/surfaces.css";

/** The workspace's third tab, which is not about a directory at all: the
 *  issues its own agents are holding (#29). A directory scopes Changes and
 *  Files; it does not scope this. */
const ISSUES_TAB = "issues";
const onIssuesTab = (route) => route.tab === ISSUES_TAB;

function mountChanges(body, { scope, callRpc, cacheScope, projectId, navigate, viewingContext, agentSelection, requestedCommit, onCommitSelection }) {
  body.innerHTML = `<div class="workspace-gitpane"></div>`;
  const refbar = document.createElement("div");
  refbar.className = "workspace-refbar";
  const gitHost = body.querySelector(".workspace-gitpane");
  let gitPane = mountGitPane(gitHost, { scope, callRpc, cacheScope, projectId, navigate, viewingContext, agentSelection,
    requestedCommit, onCommitSelection });
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
  const refPicker = mountWorkspaceRefPicker(refbar, { scope, callRpc, cacheScope, onCheckout: async () => {
      if (disposed) return;
      gitPane.dispose();
      onCommitSelection?.(null);
      gitPane = mountGitPane(gitHost, { scope, callRpc, cacheScope, projectId, navigate, viewingContext, agentSelection,
        requestedCommit: null, onCommitSelection });
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
  state.retryRefreshPending && selectedDirectory(state.workspace, state.route.sourceId)?.status !== "ready";

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
  state.paintTabs?.();
  state.onWorkspaceCached?.(workspace);
}

async function writeGitResult(state, sourceId, answer) {
  const address = gitOptionsAddress(state, sourceId);
  if (!address || !workspaceSourceIsActive(state, sourceId)) return;
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
  if (!options || !workspaceSourceIsActive(state, sourceId)) return;
  state.sourceGit = options.source?.is_git !== false;
  state.sourceNeedsReconciliation = options.source?.needs_reconciliation === true;
  state.workspaceNeedsReconciliation = options.workspace?.needs_reconciliation === true;
  state.paintTabs?.();
}

function installWorkspaceAction(state, workspace) {
  if (workspace?.status !== "failed") return;
  let current = workspace;
  let host = null;
  let pending = false;
  let message = "";
  let failed = false;
  const render = (target) => {
    host = target;
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
        state.retryRefreshPending = true;
        await writeWorkspaceResult(state, answer.workspace || answer);
        message = current?.status === "ready" ? "Workspace ready." : "Retry finished.";
      } catch (error) {
        if (state.disposed) return;
        failed = true;
        message = error.message || String(error);
      } finally {
        state.retryRefreshPending = false;
        pending = false;
        if (!state.disposed) render(host);
      }
    };
  };
  state.onWorkspaceCached = (workspaceRow) => {
    current = workspaceRow;
    if (host?.isConnected) render(host);
  };
  state.toolbarAction = render;
  setToolbarVerb(render);
}

function mountDirectoryPane(body, options) {
  const { canonical } = options;
  if (onIssuesTab(canonical)) return mountIssuesTab(body, options);
  return mountCheckoutPane(body, options);
}

/**
 * Say which issue is on screen, so the agent beside it knows what the reader
 * is looking at (#21, #29).
 *
 * The WORKSPACE half of the stamp is already there: the rail is standing on
 * this workspace, so anything sent from here to the project's agent wears the
 * workspace item (core/agentRail.js). This adds the issue to it, from the READ
 * rather than from the route — an agent told an id and nothing else is no
 * better off — and only where the bridge takes the kind at all.
 */
const sayWhichIssue = (deviceId) => (issue) => {
  const item = issueContextItem(issue, deviceId);
  if (item) App.viewingContext?.set?.({ version: 1, items: [item] });
};

function mountIssuesTab(body, { canonical, callRpc, context, feed, agentSelection }) {
  return mountWorkspaceIssuesTab(body, {
    route: canonical,
    context: context || { deviceId: canonical.deviceId, rpc: callRpc },
    feed,
    selection: agentSelection,
    sayWhichIssue: sayWhichIssue(canonical.deviceId),
    navigate: go,
  });
}

function mountCheckoutPane(body, { directory, canonical, scope, callRpc, cacheScope, agentSelection }) {
  const navigate = { openFile: ({ path, line }) => go({ ...canonical, tab: "files", file: path, line }) };
  const onCommitSelection = (commit) => {
    const route = { ...App.route };
    delete route.commit;
    if (commit) route.commit = commit;
    markRoute(route);
  };
  if (canonical.tab !== "files") {
    return mountChanges(body, {
      scope,
      callRpc,
      cacheScope,
      agentSelection,
      projectId: canonical.projectId,
      navigate,
      viewingContext: App.viewingContext,
      requestedCommit: canonical.commit || null,
      onCommitSelection,
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
      cacheScope: state.context.cacheScope,
      isActive: () => workspaceSourceIsActive(state, sourceId),
      onUpdate: async (answer) => {
        if (answer.workspace) await writeWorkspaceResult(state, answer.workspace);
        await writeGitResult(state, sourceId, answer);
        const address = gitOptionsAddress(state, sourceId);
        return address ? (await readCached(address))?.value : null;
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
    state.sourceProbePending = false;
  }
}

/** The pane's list column — the commit rail or the file tree — whichever tab is
 *  mounted. It is where the git-initialization offer hangs, and it is rebuilt
 *  from scratch whenever the pane repaints. */
const paneListColumn = (body) => body.querySelector(".crail-host, .ftree");

/** Hang the git-initialization offer off the pane's list column. */
function gitOfferPainter(body, state, sourceId) {
  return () => {
    const directory = selectedDirectory(state.workspace, sourceId);
    if (!directory) return false;
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
  // The issues tab is the workspace's, not the checkout's, so it keeps the
  // route's own tab rather than being coerced to one of the directory's two.
  const canonical = onIssuesTab(state.route)
    ? { ...state.route, sourceId }
    : { ...state.route, sourceId, tab: directoryTab(directory, state.route.tab) };
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
    context: state.context,
    // Read at every use, never captured: a workspace gains and loses agents
    // while the tab stands there, and the issues follow them.
    feed: () => deviceFeedNow(state.route.deviceId),
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

  const canonical = onIssuesTab(route)
    ? { ...route, sourceId }
    : { ...route, sourceId, tab: directoryTab(directory, route.tab) };
  // The issues tab is not about a directory, so none is written into its URL —
  // the sourceId on the canonical route is only there for the console's scope.
  if (!onIssuesTab(route) && (route.sourceId !== sourceId || route.tab !== canonical.tab)) markRoute(canonical);

  state.refreshPane = (nextWorkspace) => refreshWorkspacePane(state, nextWorkspace);
  const mounted = state.refreshPane(workspace);
  if (state.disposed) {
    state.pane?.dispose?.();
    return;
  }

  state.rail.paint(mounted.canonical.tab);
  state.paintTabs = gitOfferPainter(mounted.body, state, sourceId);
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
  const state = { selection: shellSelection(), route, context, callRpc: context.rpc, disposed: false, pane: null, toolbarAction: null, refreshPane: null, workspace: null, workspaceNeedsReconciliation: false, sourceGit: null, sourceNeedsReconciliation: false, sourceProbePending: false, paintTabs: null, rail: null, needsInitHost: false, gitInitialization: [], unwatchFeed: null, workspaceRead: null, gitOptionsRead: null, gitOptionsRevision: new Map(), retryRefreshPending: false };
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
    onSelect: (tab) => go(tab === ISSUES_TAB ? workspaceIssuesPlace(App.route) : { ...App.route, tab }),
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
    if (state.disposed) return;
    // The Issues count on the rail reads the same roster.
    state.rail.feedMoved();
    if (state.workspace) {
      // The workspace Issues pane derives its scope from the cached feed's
      // roster. Keep the cache announcement wired after mount so a later row
      // write can re-scope the already cached issue catalogue.
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
