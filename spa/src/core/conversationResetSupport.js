// This per-device greeting fact survives reloads and reconnects. Before hello,
// both the cached controls and generation-aware requests use the same fact.
import { mergeCachedAtomically, readCached } from "./localCache.js";

export const conversationResetSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "conversation-reset-support", sub: "" });
export const rememberConversationResetSupport = (deviceId, capabilities) => {
  if (!deviceId) return Promise.resolve(false);
  const supported = capabilities?.conversations?.reset === true;
  return mergeCachedAtomically(conversationResetSupportAddress(deviceId), (held) =>
    held?.supported === supported ? null : { supported });
};

export async function readConversationResetSupport(deviceId) {
  if (!deviceId) return false;
  return (await readCached(conversationResetSupportAddress(deviceId)))?.value?.supported === true;
}
