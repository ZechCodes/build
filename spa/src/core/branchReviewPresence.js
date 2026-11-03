// Review ownership is a cached fact, including legacy reviews without a PR hint.
import { cachedRecords, subscribeCache } from "./localCache.js";

const REVIEW_PRESENCE_KINDS = new Set(["task-review", "tracker-summary", "tracker-task", "tracker-tasks", "tracker-tasks-query"]);

function reviewWorkspaceIds(review) {
  const ids = (review?.snapshots || []).map((snapshot) => snapshot.workspace_id).filter(Boolean);
  if (review?.workspace_id) ids.push(review.workspace_id);
  return ids;
}
function taskWorkspaceIds(value) {
  const tasks = value?.task ? [value.task] : value?.tasks || [];
  return tasks.filter((task) => task.review_summary || task.legacy_review).flatMap((task) => task.links?.workspace_ids || []);
}
function workspaceIds(records) {
  return new Set(records.flatMap(({ value }) => [...reviewWorkspaceIds(value?.review), ...taskWorkspaceIds(value)]));
}

export function watchBranchReviewPresence({ deviceId, projectId, onChange }) {
  let ids = new Set();
  let disposed = false;
  let serial = 0;
  async function read() {
    const read = ++serial;
    const records = (await Promise.all([...REVIEW_PRESENCE_KINDS].map((kind) => cachedRecords({ deviceId, entityId: projectId, kind })))).flat();
    if (disposed || read !== serial) return;
    ids = workspaceIds(records);
    onChange();
  }
  const unwatch = subscribeCache({ deviceId, entityId: projectId }, (address) => {
    if (!address.kind || REVIEW_PRESENCE_KINDS.has(address.kind)) void read();
  });
  void read();
  return { has: (id) => ids.has(id), dispose() { disposed = true; unwatch(); } };
}

export function branchWorkspaceHasReview(feed, projectId, branch, presence) {
  return (feed?.workspaces || []).some((workspace) => workspace.project_id === projectId
    && (workspace.active_review || presence.has(workspace.workspace_id || workspace.id))
    && workspace.directories?.some((directory) => directory.branch === branch));
}
