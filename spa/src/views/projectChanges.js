// Original sources share the kept directory panes, without workspace review or
// Git initialization: each pane acts on the source's actual checkout HEAD.
import { mountWorkspaceChanges } from "./workspaceChanges.js";
import { projectChangesDirectories, projectSourceContext } from "../core/projectChangesModel.js";
import { projectChangesRpc } from "../core/projectChangesRpc.js";
import { PROJECT_CHANGES_SUPPORT_KIND, readProjectChangesSupport } from "../core/projectChangesSupport.js";
import { subscribeCache } from "../core/localCache.js";

export function mountProjectChanges(host, { project, route, context, viewingContext, agentSelection, onSourceSelection, onCommitSelection, onOpenFile }) {
  host.innerHTML = '<div class="project-changes-context"><p>Original sources · shared checkout</p><p data-project-source-context></p><button type="button" class="btn mini" data-project-sync-base hidden>Sync base</button><span class="warn" data-project-sync-error role="status"></span></div><div data-project-changes-body></div>';
  const body = host.querySelector("[data-project-changes-body]");
  const note = host.querySelector("[data-project-source-context]");
  const sync = host.querySelector("[data-project-sync-base]");
  const error = host.querySelector("[data-project-sync-error]");
  let support = { projectSources: false, syncBase: false, syncBasePath: false };
  let standing = route.sourceId;
  let pane = null;
  let disposed = false;
  let pending = false;
  let visible = true;
  let lastSelection = null;
  const commits = new Map();
  const sourceOf = (id) => project?.sources?.find((source) => source.id === id)
    || (!project?.sources && id === "primary" ? { id, name: project?.name, path: project?.path } : null);
  const sourceKey = (id) => JSON.stringify([id, sourceOf(id)?.path]);
  if (route.commit && standing) commits.set(sourceKey(standing), route.commit);
  const rpcFor = (id) => projectChangesRpc(context, sourceOf(id), { projectId: route.projectId, currentProject: () => project });
  const paintContext = () => {
    const source = sourceOf(standing);
    note.textContent = source ? projectSourceContext(source) : "No project sources configured.";
    sync.hidden = !source || source.is_git === false || !support.syncBase;
    sync.disabled = pending;
  };
  const selectSource = (id) => {
    standing = id;
    error.textContent = "";
    paintContext();
    onSourceSelection(id, commits.get(sourceKey(id)) || null);
  };
  const git = (id, gatedContext) => ({
    scope: projectChangesDirectories(project, route.projectId, support.projectSources).find((dir) => dir.sourceId === id).scope,
    callRpc: rpcFor(id), cacheScope: context.cacheScope, viewingContext: gatedContext, agentSelection,
    // Project history uses the bridge's project marker (none). A source cache
    // namespace does not make this a workspace's unpublished-work review.
    requestedCommit: commits.get(sourceKey(id)) || null,
    onCommitSelection: (commit) => {
      commits.set(sourceKey(id), commit);
      if (id === standing) onCommitSelection(commit);
    },
    navigate: { openFile: ({ path, line }) => onOpenFile({ sourceId: id, path, line }) },
  });
  function reconcile() {
    const directories = projectChangesDirectories(project, route.projectId, support.projectSources);
    if (!directories.some((directory) => directory.sourceId === standing)) standing = directories[0]?.sourceId || null;
    if (!pane) pane = mountWorkspaceChanges(body, { directories, current: standing, git, viewingContext, onSelectDirectory: selectSource });
    else pane.workspaceMoved(directories);
    if (lastSelection !== sourceKey(standing)) {
      lastSelection = sourceKey(standing);
      selectSource(standing);
    }
    pane.setVisible(visible);
    paintContext();
  }
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
    reconcile();
  };
  const unwatch = subscribeCache({ deviceId: context.deviceId }, (address) => {
    if (address.kind === PROJECT_CHANGES_SUPPORT_KIND) void readSupport();
  });
  void readSupport();
  return {
    update(next) { project = next; if (pane) reconcile(); },
    setVisible(shown) { visible = shown; pane?.setVisible?.(shown); },
    dispose() { disposed = true; unwatch(); pane?.dispose(); },
  };
}
