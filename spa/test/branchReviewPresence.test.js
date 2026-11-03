import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import legacyFixture from "../../fixtures/api/v1/tasks.review.get.json";
import { stampWorkspace } from "../src/core/feedMerge.js";
import { replaceSessionList } from "../src/core/sessionListCache.js";
import { wipeCache, writeCached, readCached, cachedRecords } from "../src/core/localCache.js";
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

it("recognizes canonical retained legacy reviews and server PR hints", async () => {
  const scope = { deviceId: "branch-presence", projectId: "project" };
  const presence = watchBranchReviewPresence({ ...scope, onChange() {} });
  await writeCached({ deviceId: scope.deviceId, entityId: scope.projectId, kind: "task-review", sub: "task" }, { review: legacyFixture.result.review });
  await vi.waitFor(() => expect(presence.has(legacyFixture.result.review.workspace_id)).toBe(true));
  const feed = { workspaces: [{ id: "pr-workspace", project_id: scope.projectId, active_review: { task_id: "pr" }, directories: [{ branch: "work" }] }] };
  expect(branchWorkspaceHasReview(feed, scope.projectId, "work", presence)).toBe(true);
  expect(branchWorkspaceHasReview(feed, "other-project", "work", presence)).toBe(false);
  presence.dispose();
});

it("hides branch merge from a server workspace marker with empty task and review caches", async () => {
  const scope = { deviceId: "branch-cold", projectId: "project" };
  const changed = vi.fn();
  const presence = watchBranchReviewPresence({ ...scope, onChange: changed });
  await vi.waitFor(() => expect(changed).toHaveBeenCalled());
  const address = { deviceId: scope.deviceId, entityId: "", kind: "workspaces" };
  await replaceSessionList(address, "workspaces", [{ id: "workspace", project_id: scope.projectId,
    legacy_review: true, directories: [{ branch: "work" }] }]);
  const workspaces = (await readCached({ ...address, kind: "workspaces" })).value;
  const feed = { workspaces: workspaces.map((workspace) => stampWorkspace(workspace, scope.deviceId)) };
  expect(feed.workspaces[0].legacy_review).toBe(true);
  expect(await cachedRecords({ deviceId: scope.deviceId, entityId: scope.projectId })).toEqual([]);
  expect(branchWorkspaceHasReview(feed, scope.projectId, "work", presence)).toBe(true);
  const noReview = { workspaces: [{ ...feed.workspaces[0], legacy_review: false }] };
  expect(branchWorkspaceHasReview(noReview, scope.projectId, "work", presence)).toBe(false);
  expect(branchWorkspaceHasReview(feed, scope.projectId, "other", presence)).toBe(false);
  presence.dispose();
});
