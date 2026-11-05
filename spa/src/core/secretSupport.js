// Rendering remembers the latest greeting; dispatch still requires the live
// adapter's current answer.
import { mergeCachedAtomically, readCached } from "./localCache.js";

export const secretSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "secret-support" });

export function rememberSecretSupport(deviceId, capabilities, current = () => true) {
  if (!deviceId) return Promise.resolve(false);
  const attachments = capabilities?.secrets?.attachments === true
    && capabilities.secrets.sealedUpload === true && capabilities.secrets.metadataEvents === true;
  return mergeCachedAtomically(secretSupportAddress(deviceId), (held) =>
    current() && held?.attachments !== attachments ? { attachments } : null);
}

export async function readSecretSupport(deviceId) {
  const held = deviceId ? (await readCached(secretSupportAddress(deviceId)))?.value : null;
  return { attachments: held?.attachments === true };
}
