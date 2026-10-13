// Source assertions are checked on the device's current compatible greeting.
// Cached support is for painting only; it cannot authorize new selectors.
import { whenGreeted } from "./deviceContexts.js";
import { bridgeCapabilities } from "./changeEvents.js";
import { readCached } from "./localCache.js";

export function isProjectTerminalScope(scope) {
  return Boolean(scope?.project_id && (scope.source_id || scope.source_path));
}

export async function projectTerminalSource(context, scope) {
  const projects = (await readCached({ deviceId: context.deviceId, entityId: "", kind: "projects" }))?.value || [];
  const project = projects.find((entry) => (entry.id || entry.project_id) === scope.project_id);
  const sources = project?.sources || (project?.path ? [{ id: "primary", path: project.path }] : []);
  return sources.find((entry) => entry.id === scope.source_id && entry.path === scope.source_path);
}

export async function projectTerminalRpc(context, method, params, call = context?.rpc) {
  if (!context) throw new Error("This machine is unavailable.");
  const request = await whenGreeted(context, async (stands) => {
    if (!bridgeCapabilities(context.deviceId).term?.projectSources) {
      throw new Error("Update this device to use project source terminals.");
    }
    const source = await projectTerminalSource(context, params);
    if (!source) throw new Error("This project source moved or was removed. Reopen Terminal before using it.");
    if (!stands()) throw new Error("This machine's greeting changed before using the terminal.");
    return call(method, params);
  });
  if (!request) throw new Error("This machine is unavailable.");
  return request.sent;
}
