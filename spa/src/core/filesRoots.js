// The shared Files explorer's roots, for project sources and workspace directories.
import { workspaceDirectoryModel, workspaceScope } from "./workspaceModel.js";

const root = (id, label, scope) => ({ id, label, scope });

export const workspaceFilesRoots = (workspace, workspaceId) =>
  workspaceDirectoryModel(workspace).map((directory) =>
    root(directory.sourceId, directory.label, workspaceScope(workspaceId, directory.sourceId, workspace)));

export function projectFilesRoots(project, projectId) {
  if (!project) return [];
  if (Array.isArray(project.sources)) {
    return project.sources.map((source) => ({
      ...root(source.id, source.name || source.mount || source.id, { project_id: projectId, source_id: source.id }),
      ...(source.path ? { cacheEntityId: `project:${JSON.stringify([projectId, source.id, source.path])}` } : null),
    }));
  }
  return project.path ? [root("primary", project.name || "Files", { project_id: projectId })] : [];
}
