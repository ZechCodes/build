// Review ownership is a cached fact, including legacy reviews without a PR hint.
import { cachedRecords, subscribeCache } from "./localCache.js";

export function watchBranchReviewPresence({ deviceId, projectId, onChange }) {
  const address = { deviceId, entityId: projectId, kind: "task-review" };
  let ids = new Set();
  let disposed = false;
  let serial = 0;
  async function read() {
    const read = ++serial;
    const records = await cachedRecords(address);
    if (disposed || read !== serial) return;
    ids = new Set(records.map(({ value }) => value?.review?.workspace_id).filter(Boolean));
    onChange();
  }
  const unwatch = subscribeCache(address, () => void read());
  void read();
  return { has: (id) => ids.has(id), dispose() { disposed = true; unwatch(); } };
}

export function branchWorkspaceHasReview(feed, projectId, branch, presence) {
  return (feed?.workspaces || []).some((workspace) => workspace.project_id === projectId
    && (workspace.legacy_review === true || workspace.active_review || presence.has(workspace.workspace_id || workspace.id))
    && workspace.directories?.some((directory) => directory.branch === branch));
}
