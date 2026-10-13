import { directoryCacheId } from "./directoryScope.js";
import { mergeCachedTogether } from "./localCache.js";
import { notePush, pushFence, pushedSince } from "./pushFence.js";

export function projectTerminalScopes(projects) {
  return (projects || []).flatMap((project) => {
    const sources = project.sources || (project.path ? [{ id: "primary", path: project.path }] : []);
    return sources.map((source) => ({ project_id: project.id || project.project_id, source_id: source.id, source_path: source.path }));
  });
}

export function hasProjectTerminalSource(projects, scope) {
  return projectTerminalScopes(projects).some((entry) => ["project_id", "source_id", "source_path"].every((field) => entry[field] === scope[field]));
}

export async function writeProjectTerminalRoster(context, scope, tabs, { fence = pushFence(), pushed = false } = {}) {
  const address = { deviceId: context.deviceId, entityId: directoryCacheId(scope), kind: "terminals" };
  if (pushed) notePush(address);
  const projects = { deviceId: context.deviceId, entityId: "", kind: "projects" };
  return mergeCachedTogether([projects, address], ([held]) => {
    if (!context.active() || !hasProjectTerminalSource(held, scope)) return null;
    if (!pushed && pushedSince(address, fence)) return null;
    return [null, { tabs }];
  });
}
