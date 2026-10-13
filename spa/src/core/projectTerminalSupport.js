import { mergeCachedAtomically, readCached } from "./localCache.js";
export const PROJECT_TERMINAL_SUPPORT_KIND = "project-terminal-support";
const address = (deviceId) => ({ deviceId, entityId: "", kind: PROJECT_TERMINAL_SUPPORT_KIND });
export function rememberProjectTerminalSupport(deviceId, capabilities) {
  if (!deviceId) return Promise.resolve(false);
  const next = { projectSources: capabilities?.term?.projectSources === true };
  return mergeCachedAtomically(address(deviceId), (held) => held?.projectSources === next.projectSources ? null : next);
}
export async function readProjectTerminalSupport(deviceId) {
  const held = deviceId ? (await readCached(address(deviceId)))?.value : null;
  return { projectSources: held?.projectSources === true };
}
