import { workspaceFailedText } from "./text.js";

export function directoryId(directory) {
  return directory?.source_id || directory?.id || null;
}

export function selectedDirectory(workspace, sourceId) {
  const directories = workspace?.directories || [];
  return directories.find((entry) => directoryId(entry) === sourceId) || directories[0] || null;
}

/** The source directories mounted into a workspace, in its own order — the
 * rows its Files roots and its Changes tabs are drawn from. `source_id` is
 * the stable route identity; `id` is the concrete workspace-directory record
 * and remains available for RPCs that need it. */
export function workspaceDirectoryModel(workspace, sourceId = null) {
  return ((workspace && workspace.directories) || []).map((directory) => ({
    ...directory,
    sourceId: directory.source_id || directory.id,
    label: directory.name || directory.mount || directory.source_id || directory.id,
    current: (directory.source_id || directory.id) === sourceId,
  }));
}

/** Which tab a workspace directory can be standing on. A directory with no
 * repository in it has no Changes to show, so Files is the only surface it
 * has — whatever the URL, the row or the menu asked for. Everything that mints
 * a workspace route or opens a directory asks here, so the rule is one rule. */
export const directoryTab = (directory, wanted = "changes") =>
  directory?.is_git === false ? "files" : wanted || "changes";

/** The last segment of a checkout's path — the folder the bridge made for it.
 *  Trailing separators are dropped first, so a root recorded with one still
 *  names its own folder rather than nothing. */
const folderOf = (root) => String(root || "").replace(/[/\\]+$/, "").split(/[/\\]/).pop() || "";

/**
 * What a workspace is called, everywhere the account reads one: the name whoever
 * made it typed.
 *
 * That name is user-facing text the bridge keeps byte for byte, while the
 * checkout's folder and its branch are bounded, portable slugs DERIVED from it —
 * "Bridge wire interface" lives in `bridge-wire-interface/` on
 * `build/bridge-wire-interface`. None of those derivatives is the workspace's
 * name, and a surface that shows one is showing the reader the machinery instead
 * of what they called their work. So the rail's rows, their tooltips, the
 * project blocks and the toolbar's switcher all ask here.
 *
 * An empty name is a name — the user cleared it, and the row says what they
 * said — which is why the name is taken whenever the record HAS one. Only a
 * workspace no bridge ever named (an older bridge, an adopted checkout) falls
 * back, and it falls back to its folder rather than to the whole path: a row is
 * one line, and a path is not a name.
 */
export const workspaceDisplayName = (workspace, fallback = "Workspace") =>
  workspace?.name ?? (folderOf(workspace?.root) || fallback);

/** Why a workspace failed, as its bridge told it: the checkout is one build per
 * directory, and the first directory that could not be made carries the
 * message. Empty when the bridge said nothing — a failure is still a failure. */
const workspaceFailure = (workspace) =>
  (workspace?.directories || []).map((directory) => directory.error).find(Boolean) || "";

/** What a workspace's state says wherever it is listed — the rail's rows and
 * the toolbar's switcher. The bridge's own word for every state it reports; for
 * a checkout it could not build, the one failure wording (core/text.js), so the
 * two lists never disagree about the same workspace. */
export const workspaceStatusText = (workspace) =>
  (workspace?.status === "failed" ? workspaceFailedText(workspaceFailure(workspace)) : workspace?.status) || "";

/** Whether a workspace stands on the project's own checkout — the repository
 * the project was registered at, which every workspace is cut FROM. A template
 * is not a place to work, so nothing lists it: the bridge stopped answering it
 * in `workspace.list`, and a machine still running a bridge that does (as
 * `legacy-<project>`) has it kept out here, by the one fact that names it. The
 * project is looked up by the caller, since the two halves of the app hold
 * their projects differently. */
export const standsOnProjectCheckout = (workspace, project) =>
  !!workspace?.root && !!project?.path && workspace.root === project.path;

export function workspaceScope(workspaceId, sourceId, workspace = null) {
  if (!workspaceId || !sourceId) return null;
  const scope = { workspace_id: workspaceId, source_id: sourceId };
  const entityId = ownGitEntity(workspace, sourceId);
  if (entityId) scope.entity_id = entityId;
  return scope;
}

/** The entity a directory's git is filed under, or null. The workspace's own
 *  git directory — the first git one — is the checkout its conversation's run
 *  stands on, so a pane over it files its git under that entity
 *  (core/directoryScope.js). Every other directory keeps a namespace of its own. */
function ownGitEntity(workspace, sourceId) {
  const own = (workspace?.directories || []).find((directory) => directory.is_git);
  if (!own || !workspace?.entity_id) return null;
  return own.source_id === sourceId || own.id === sourceId ? workspace.entity_id : null;
}

/** The legacy active run whose checkout is this adopted workspace. Paths are
 * daemon-owned canonical strings; matching both project and root avoids branch
 * aliases and same-named checkouts — and the match is narrowed to the machine
 * the workspace is on, because a path on one machine says nothing about the
 * same path on another. */
export function workspaceRun(workspace, items = []) {
  if (!workspace?.root) return null;
  const terminal = new Set(["merged", "abandoned", "archived", "done", "finished"]);
  const matches = items.filter((item) =>
    item.kind === "branch"
      && item.deviceId === workspace.deviceId
      && item.project_id === workspace.project_id
      && item.run_id
      && item.worktree_path === workspace.root
      && !terminal.has(item.state)
      && !terminal.has(item.status),
  );
  return matches.length === 1 ? matches[0] : null;
}

export function refLabel(current) {
  if (!current) return "Select ref";
  return current.kind === "detached" ? `Detached at ${current.commit || "unknown"}` : current.name || "Select ref";
}
