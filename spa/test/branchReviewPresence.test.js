import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import legacyFixture from "../../fixtures/api/v1/tasks.review.get.json";
import { stampWorkspace } from "../src/core/feedMerge.js";
import { replaceSessionList } from "../src/core/sessionListCache.js";
import { wipeCache, writeCached, readCached, cachedRecords } from "../src/core/localCache.js";
import { branchWorkspaceHasReview, watchBranchReviewPresence } from "../src/core/branchReviewPresence.js";

beforeEach(wipeCache);
it.each([{ legacy_review: true }, { review_summary: { mode: "pull_request", workspace_id: "workspace-1" } }])(
  "keeps a merely linked workspace mergeable with cold and cached task records: %j", async (marker) => {
    const scope = { deviceId: "branch-presence", projectId: "project" };
    const onChange = vi.fn();
    const presence = watchBranchReviewPresence({ ...scope, onChange });
    const owner = legacyFixture.result.review.workspace_id;
    const unrelated = "workspace-unrelated";
    const feed = { workspaces: [
      { id: owner, project_id: scope.projectId, legacy_review: true, directories: [{ branch: "owner" }] },
      { id: unrelated, project_id: scope.projectId, directories: [{ branch: "unrelated" }] },
    ] };
    await vi.waitFor(() => expect(onChange).toHaveBeenCalled());
    expect(branchWorkspaceHasReview(feed, scope.projectId, "owner", presence)).toBe(true);
    expect(branchWorkspaceHasReview(feed, scope.projectId, "unrelated", presence)).toBe(false);
    const task = { id: legacyFixture.result.review.task_id, ...marker, links: { workspace_ids: [owner, unrelated] } };
    for (const kind of ["tracker-summary", "tracker-task", "tracker-tasks", "tracker-tasks-query"]) {
      await writeCached({ deviceId: scope.deviceId, entityId: scope.projectId, kind, sub: task.id },
        kind === "tracker-tasks" || kind === "tracker-tasks-query" ? { tasks: [task] } : { task });
    }
    await writeCached({ deviceId: scope.deviceId, entityId: scope.projectId, kind: "task-review", sub: task.id }, { review: legacyFixture.result.review });
    await vi.waitFor(() => expect(presence.has(owner)).toBe(true));
    expect(presence.has(unrelated)).toBe(false);
    expect(branchWorkspaceHasReview(feed, scope.projectId, "unrelated", presence)).toBe(false);
    expect(branchWorkspaceHasReview(feed, scope.projectId, "owner", presence)).toBe(true);
    presence.dispose();
  },
);

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
