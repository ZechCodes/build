export function directoryId(directory) {
  return directory?.source_id || directory?.id || null;
}

export function selectedDirectory(workspace, sourceId) {
  const directories = workspace?.directories || [];
  return directories.find((entry) => directoryId(entry) === sourceId) || directories[0] || null;
}

/** Which tab a workspace directory can be standing on. A directory with no
 * repository in it has no Changes to show, so Files is the only surface it
 * has — whatever the URL, the row or the menu asked for. Everything that mints
 * a workspace route or opens a directory asks here, so the rule is one rule. */
export const directoryTab = (directory, wanted = "changes") =>
  directory?.is_git === false ? "files" : wanted || "changes";

export function workspaceScope(workspaceId, sourceId) {
  return workspaceId && sourceId ? { workspace_id: workspaceId, source_id: sourceId } : null;
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
