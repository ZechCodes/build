// Source addressing is remembered for cold renders, and checked again at dispatch.
import { mergeCachedAtomically, readCached } from "./localCache.js";

export const PROJECT_FILES_SUPPORT_KIND = "project-files-support";
const address = (deviceId) => ({ deviceId, entityId: "", kind: PROJECT_FILES_SUPPORT_KIND });

export function rememberProjectFilesSupport(deviceId, capabilities) {
  if (!deviceId) return Promise.resolve(false);
  const projectSources = capabilities?.fs?.projectSources === true;
  return mergeCachedAtomically(address(deviceId), (held) => held?.projectSources === projectSources ? null : { projectSources });
}

export async function readProjectFilesSupport(deviceId) {
  return deviceId ? (await readCached(address(deviceId)))?.value?.projectSources === true : false;
}

