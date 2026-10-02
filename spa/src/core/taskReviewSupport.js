// Greetings write capabilities once; review surfaces paint this cached fact.
import { mergeCachedAtomically, readCached } from "./localCache.js";

export const NO_REVIEW_SUPPORT = Object.freeze({ get: false, snapshot: false, diff: false, complete: false, act: false, comments: false });
export const reviewSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "task-review-support" });

export function rememberReviewSupport(deviceId, capabilities, current = () => true) {
  const next = { ...NO_REVIEW_SUPPORT, ...capabilities?.reviews };
  return mergeCachedAtomically(reviewSupportAddress(deviceId), (held) =>
    !current() || JSON.stringify(held) === JSON.stringify(next) ? null : next);
}

export async function readReviewSupport(deviceId) {
  return (await readCached(reviewSupportAddress(deviceId)))?.value || NO_REVIEW_SUPPORT;
}
