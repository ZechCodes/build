import { syncStatusLine } from "./sourceSyncModel.js";
export function projectChangesDirectories(project, projectId, supported) {
  const sources = project?.sources || (project?.path ? [{ id: "primary", name: project.name, path: project.path }] : []);
  return sources.map((source, index) => ({
    sourceId: source.id,
    label: source.name || source.mount || source.id,
    is_git: source.is_git,
    scope: { project_id: projectId, source_id: source.id, source_path: source.path },
    ...(!supported && index > 0 && source.is_git !== false
      ? { status: "unsupported", error: "Update this device to use Changes for this project source." } : {}),
  }));
}
export function projectSourceContext(source, now = Date.now()) {
  if (!source || source.is_git === false) return "This source is not a Git repository.";
  const base = source.base_branch ? `Base ${source.base_branch}` : "No configured base";
  const setting = source.sync_base ? "Automatic base sync on" : "Automatic base sync off";
  // Show the last manual outcome even when automatic sync is off.
  const outcome = syncStatusLine({ ...source, sync_base: true }, now);
  return `${base} · ${setting} · ${outcome}`;
}
