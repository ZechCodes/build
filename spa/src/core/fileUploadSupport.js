import { mergeCachedAtomically, readCached } from "./localCache.js";

export const fileUploadSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "file-upload-support" });
const supportOf = (fs) => ({ uploads: fs?.uploads === true, createDirectory: fs?.createDirectory === true });

export function rememberFileUploadSupport(deviceId, capabilities) {
  if (!deviceId) return Promise.resolve(false);
  const support = supportOf(capabilities?.fs);
  return mergeCachedAtomically(fileUploadSupportAddress(deviceId), (held) =>
    held?.uploads === support.uploads && held?.createDirectory === support.createDirectory ? null : support);
}

export async function readFileUploadSupport(deviceId) {
  const record = deviceId ? await readCached(fileUploadSupportAddress(deviceId)) : null;
  return supportOf(record?.value);
}
