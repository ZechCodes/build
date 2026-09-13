export function directoryId(directory) {
  return directory?.source_id || directory?.id || null;
}

export function selectedDirectory(workspace, sourceId) {
  const directories = workspace?.directories || [];
  return directories.find((entry) => directoryId(entry) === sourceId) || directories[0] || null;
}

export function workspaceScope(workspaceId, sourceId) {
  return workspaceId && sourceId ? { workspace_id: workspaceId, source_id: sourceId } : null;
}

/** The legacy active run whose checkout is this adopted workspace. Paths are
 * daemon-owned canonical strings; matching both project and root avoids branch
 * aliases and same-named checkouts. */
export function workspaceRun(workspace, items = []) {
  if (!workspace?.root) return null;
  const terminal = new Set(["merged", "abandoned", "archived", "done", "finished"]);
  const matches = items.filter((item) =>
    item.kind === "branch"
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
