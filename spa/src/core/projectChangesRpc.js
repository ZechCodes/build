import { whenGreeted } from "./deviceContexts.js";
import { bridgeCapabilities } from "./changeEvents.js";

function sourceIn(project, source) {
  if (project?.sources) return project.sources.find((entry) => entry.id === source.id);
  return source.id === "primary" && project?.path ? { ...source, path: project.path } : null;
}
function currentSource(source, owner) {
  const project = owner.currentProject();
  const current = sourceIn(project, source);
  if (!current || current.path !== source.path) throw new Error("This project source moved or was removed. Reopen Changes before changing it.");
  return { current, primary: !project.sources || project.sources[0]?.id === source.id };
}
function syncParams(source, owner, capabilities, params) {
  if (!capabilities.projects?.syncBase) throw new Error("Update this device to sync the configured base.");
  return { ...params, project_id: owner.projectId, source_id: source.id,
    ...(capabilities.projects.syncBasePath ? { source_path: source.path } : {}) };
}
function gitParams(source, owner, capabilities, params, primary) {
  const { source_id: _id, source_path: _path, ...legacy } = params;
  if (capabilities.git?.projectSources) return { ...legacy, project_id: owner.projectId, source_id: source.id, source_path: source.path };
  if (!primary) throw new Error("Update this device to use project Changes.");
  return { ...legacy, project_id: owner.projectId };
}
export function projectChangesRpc(context, source, owner) {
  return async (method, params, ...options) => {
    const request = await whenGreeted(context, () => {
      const { primary } = currentSource(source, owner);
      const capabilities = bridgeCapabilities(context.deviceId);
      const scoped = method === "project.sync_source"
        ? syncParams(source, owner, capabilities, params)
        : gitParams(source, owner, capabilities, params, primary);
      return context.rpc(method, scoped, ...options);
    });
    if (!request) throw new Error("This machine is unavailable.");
    return request.sent;
  };
}
