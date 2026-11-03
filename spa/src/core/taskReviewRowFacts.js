// Legacy reviews have no compact PR summary. Read only cached metadata to keep
// their identity visible in task rows without fetching a review per row.
import { cachedRecords } from "./localCache.js";

export const reviewRowsAddress = (deviceId, projectId) => ({ deviceId, entityId: projectId, kind: "task-review" });

export async function readReviewRowFacts(deviceId, projectId, support = {}) {
  if (!support.reject) return new Map();
  const records = await cachedRecords(reviewRowsAddress(deviceId, projectId));
  return new Map(records.filter(({ value }) => value?.review)
    .map(({ address, value }) => [address.sub, value.review]));
}
