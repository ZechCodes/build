import { directoryId } from "./workspaceModel.js";

/** Conversation paths are relative to the workspace root, while Changes paths
 * are relative to the selected source checkout. Use actual mount paths. */
export function workspaceCommentMessages(messages, workspace, sourceId) {
  const source = workspace.directories?.find((directory) => directoryId(directory) === sourceId);
  const root = workspace.root?.replace(/\/+$/, "");
  const path = source?.path?.replace(/\/+$/, "");
  if (!root || !path || (path !== root && !path.startsWith(`${root}/`))) {
    throw new Error("Workspace source directory is unavailable");
  }
  const prefix = path === root ? "" : `${path.slice(root.length + 1)}/`;
  const qualify = (item) => item?.path ? { ...item, path: prefix + item.path } : item;
  return messages.map((message) => ({
    ...message,
    ...(message.anchor ? { anchor: qualify(message.anchor) } : {}),
    ...(message.viewing_context ? {
      viewing_context: { ...message.viewing_context, items: message.viewing_context.items.map(qualify) },
    } : {}),
  }));
}
