// A greeting records support; cold views paint this cached contract.
import { mergeCachedAtomically, readCached } from "./localCache.js";
export const PROJECT_CHANGES_SUPPORT_KIND = "project-changes-support";
const address = (deviceId) => ({ deviceId, entityId: "", kind: PROJECT_CHANGES_SUPPORT_KIND });
const supportOf = (capabilities) => ({
  projectSources: capabilities?.git?.projectSources === true,
  syncBase: capabilities?.projects?.syncBase === true,
  syncBasePath: capabilities?.projects?.syncBasePath === true,
});
export function rememberProjectChangesSupport(deviceId, capabilities) {
  if (!deviceId) return Promise.resolve(false);
  const next = supportOf(capabilities);
  return mergeCachedAtomically(address(deviceId), (held) => JSON.stringify(held) === JSON.stringify(next) ? null : next);
}
export async function readProjectChangesSupport(deviceId) {
  const held = deviceId ? (await readCached(address(deviceId)))?.value : null;
  return { projectSources: held?.projectSources === true, syncBase: held?.syncBase === true, syncBasePath: held?.syncBasePath === true };
}
