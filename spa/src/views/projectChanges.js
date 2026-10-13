// Original sources share the kept directory panes, without workspace review or
// Git initialization: each pane acts on the source's actual checkout HEAD.
import { mountWorkspaceChanges } from "./workspaceChanges.js";
import { projectChangesDirectories, projectSourceContext } from "../core/projectChangesModel.js";
import { projectChangesRpc } from "../core/projectChangesRpc.js";
import { PROJECT_CHANGES_SUPPORT_KIND, readProjectChangesSupport } from "../core/projectChangesSupport.js";
import { subscribeCache } from "../core/localCache.js";
import { mountProjectChangesDrafts } from "../core/projectChangesDrafts.js";

export function mountProjectChanges(host, { project, route, context, viewingContext, agentSelection, onSourceSelection, onCommitSelection, onOpenFile }) {
  host.innerHTML = '<div class="project-changes-context"><p>Original sources · shared checkout</p><p data-project-source-context></p><button type="button" class="btn mini" data-project-sync-base hidden>Sync base</button><span class="warn" data-project-sync-error role="status"></span><div class="warn" data-project-missing-source role="status" hidden><span data-project-link-status></span> <button type="button" class="btn mini" data-project-use-source hidden></button></div></div><div class="project-changes-drafts" data-project-changes-drafts hidden></div><div data-project-changes-body></div>';
  const drafts = mountProjectChangesDrafts(host.querySelector("[data-project-changes-drafts]"), { deviceId: context.deviceId, project });
  const body = host.querySelector("[data-project-changes-body]");
  const note = host.querySelector("[data-project-source-context]");
  const sync = host.querySelector("[data-project-sync-base]");
  const error = host.querySelector("[data-project-sync-error]");
  const missingSource = host.querySelector("[data-project-missing-source]");
  const linkStatus = host.querySelector("[data-project-link-status]");
  const useSource = host.querySelector("[data-project-use-source]");
  let support = { projectSources: false, syncBase: false, syncBasePath: false };
  let standing = route.sourceId;
  let pane = null;
  let disposed = false;
  let pending = false;
  let visible = true;
  let lastSelection = null;
  let supportLoaded = false;
  let reconciling = false;
  const commits = new Map();
  let defaultCommit = !route.sourceId ? route.commit : null;
  let requestedSource = route.sourceId ? { id: route.sourceId, commit: route.commit } : null;
  const sourceOf = (id) => project?.sources?.find((source) => source.id === id)
    || (!project?.sources && id === "primary" ? { id, name: project?.name, path: project?.path } : null);
  const sourceKey = (id) => JSON.stringify([id, sourceOf(id)?.path]);
  const resolveRequestedSource = () => {
    if (!requestedSource) return true;
    if (!sourceOf(requestedSource.id)?.path) return false;
    standing = requestedSource.id;
    if (requestedSource.commit) commits.set(sourceKey(standing), requestedSource.commit);
    requestedSource = null;
    return true;
  };
  const rpcFor = (id) => projectChangesRpc(context, sourceOf(id), { projectId: route.projectId, currentProject: () => project });
  const paintRequestedSource = (source) => {
    missingSource.hidden = !requestedSource;
    useSource.hidden = !requestedSource || !source;
    useSource.textContent = source ? `Use ${source.name || source.id}` : "";
    if (!requestedSource) return;
    linkStatus.textContent = project?.sources?.length
      ? `Requested source “${requestedSource.id}” is unavailable. Choose another source to continue; its linked commit will not be opened there.`
      : `Waiting for project source “${requestedSource.id}”.`;
  };
  const paintContext = () => {
    const source = sourceOf(standing);
    note.textContent = source ? projectSourceContext(source) : "No project sources configured.";
    sync.hidden = !source || source.is_git === false || !support.syncBase;
    sync.disabled = pending;
    paintRequestedSource(source);
  };
  const selectSource = (id) => {
    if (!reconciling) requestedSource = null;
    standing = id;
    error.textContent = "";
    paintContext();
    if (!requestedSource && !reconciling) onSourceSelection(id, commits.get(sourceKey(id)) || null);
  };
  const git = (id, gatedContext) => ({
    scope: projectChangesDirectories(project, route.projectId, support.projectSources).find((dir) => dir.sourceId === id).scope,
    callRpc: rpcFor(id), cacheScope: context.cacheScope, viewingContext: gatedContext, agentSelection,
    // Project history uses the bridge's project marker (none). A source cache
    // namespace does not make this a workspace's unpublished-work review.
    requestedCommit: commits.get(sourceKey(id)) || null,
    onCommitSelection: (commit) => {
      commits.set(sourceKey(id), commit);
      if (id === standing && !requestedSource) onCommitSelection(commit);
    },
    navigate: { openFile: ({ path, line }) => onOpenFile({ sourceId: id, path, line }) },
  });
  const reconcilePane = (directories, selectRequested) => {
    reconciling = true;
    try {
      if (!pane) pane = mountWorkspaceChanges(body, { directories, current: standing, git, viewingContext, onSelectDirectory: selectSource });
      else {
        pane.workspaceMoved(directories);
        if (selectRequested) pane.selectDirectory(selectRequested);
      }
    } finally { reconciling = false; }
  };
  const publishSelection = () => {
    if (requestedSource || lastSelection === sourceKey(standing)) return;
    lastSelection = sourceKey(standing);
    selectSource(standing);
  };
  const resolveDefaultCommit = () => {
    if (!defaultCommit || !standing) return;
    commits.set(sourceKey(standing), defaultCommit);
    defaultCommit = null;
  };
  function reconcile() {
    const requested = requestedSource?.id;
    const resolved = resolveRequestedSource();
    const directories = projectChangesDirectories(project, route.projectId, support.projectSources)
      .filter((directory) => directory.scope.source_path);
    if (!resolved && !directories.length && !pane) { paintContext(); return; }
    if (!directories.some((directory) => directory.sourceId === standing)) standing = directories[0]?.sourceId || null;
    resolveDefaultCommit();
    reconcilePane(directories, resolved && requested);
    publishSelection();
    pane.setVisible(visible);
    paintContext();
  }
  useSource.onclick = () => selectSource(standing);
  sync.onclick = async () => {
    if (pending || !sourceOf(standing)) return;
    pending = true;
    error.textContent = "";
    const rpc = rpcFor(standing);
    paintContext();
    try { await rpc("project.sync_source", {}); }
    catch (failure) { if (!disposed) error.textContent = failure.message || String(failure); }
    finally { pending = false; if (!disposed) paintContext(); }
  };
  let revision = 0;
  const readSupport = async () => {
    const read = ++revision;
    const next = await readProjectChangesSupport(context.deviceId);
    if (disposed || read !== revision) return;
    support = next;
    supportLoaded = true;
    reconcile();
  };
  const unwatch = subscribeCache({ deviceId: context.deviceId }, (address) => {
    if (address.kind === PROJECT_CHANGES_SUPPORT_KIND) void readSupport();
  });
  void readSupport();
  return {
    update(next) { project = next; drafts.update(next); if (supportLoaded) reconcile(); },
    setVisible(shown) { visible = shown; pane?.setVisible?.(shown); },
    dispose() { disposed = true; unwatch(); drafts.dispose(); pane?.dispose(); },
  };
}
