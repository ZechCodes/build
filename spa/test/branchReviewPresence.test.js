import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { branchWorkspaceHasReview, watchBranchReviewPresence } from "../src/core/branchReviewPresence.js";

beforeEach(wipeCache);
it("recognizes PR hints and legacy saved reviews and updates when their compact marker arrives", async () => {
  const scope = { deviceId: "branch-presence", projectId: "project" };
  const onChange = vi.fn();
  const presence = watchBranchReviewPresence({ ...scope, onChange });
  const feed = { workspaces: [{ id: "workspace", project_id: scope.projectId, directories: [{ branch: "work" }] }] };
  await vi.waitFor(() => expect(onChange).toHaveBeenCalled());
  expect(branchWorkspaceHasReview(feed, scope.projectId, "work", presence)).toBe(false);
  await writeCached({ deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-summary", sub: "task" }, { task: { legacy_review: true, links: { workspace_ids: ["workspace"] } } });
  await vi.waitFor(() => expect(branchWorkspaceHasReview(feed, scope.projectId, "work", presence)).toBe(true));
  expect(branchWorkspaceHasReview(feed, scope.projectId, "other", presence)).toBe(false);
  presence.dispose();
});

it("recognizes retained legacy snapshots and server PR hints", async () => {
  const scope = { deviceId: "branch-presence", projectId: "project" };
  const presence = watchBranchReviewPresence({ ...scope, onChange() {} });
  await writeCached({ deviceId: scope.deviceId, entityId: scope.projectId, kind: "task-review", sub: "task" }, { review: { snapshots: [{ workspace_id: "legacy-workspace" }] } });
  await vi.waitFor(() => expect(presence.has("legacy-workspace")).toBe(true));
  const feed = { workspaces: [{ id: "pr-workspace", project_id: scope.projectId, active_review: { task_id: "pr" }, directories: [{ branch: "work" }] }] };
  expect(branchWorkspaceHasReview(feed, scope.projectId, "work", presence)).toBe(true);
  expect(branchWorkspaceHasReview(feed, "other-project", "work", presence)).toBe(false);
  presence.dispose();
});
