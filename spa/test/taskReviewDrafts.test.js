import { beforeEach, expect, it, vi } from "vitest";
import "fake-indexeddb/auto";
import { evictEntity, wipeCache } from "../src/core/localCache.js";
import { readUiRecord, wipeUiRecords } from "../src/core/localUiStore.js";
import {
  reviewActionDraftAddress, reviewCreateDraftAddress, watchReviewActionDraft, watchReviewCreateDraft,
} from "../src/core/taskReviewDrafts.js";
import { createTaskReviewRepository } from "../src/core/taskReviewCache.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import openFixture from "../../fixtures/api/v1/tasks.review.open.json";
import mergeFixture from "../../fixtures/api/v1/tasks.review.merge.json";
import getFixture from "../../fixtures/api/v1/tasks.review.get.json";

vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: () => null }));

const scope = { deviceId: "pr-drafts", projectId: "project-1", workspaceId: "workspace-1", taskId: "task-1", snapshotId: "snapshot-1" };
beforeEach(async () => { await wipeCache(); await wipeUiRecords(); });

it("restores the exact create request from build-ui after replica eviction and a reload", async () => {
  const draft = watchReviewCreateDraft(scope, () => {});
  await draft.ready;
  draft.schedule(openFixture.params);
  await draft.flush();
  draft.dispose();
  await evictEntity(scope.deviceId, scope.projectId);
  await wipeCache();
  const paint = vi.fn();
  const restored = watchReviewCreateDraft(scope, paint);
  expect(await restored.ready).toEqual(openFixture.params);
  expect(paint).toHaveBeenCalledExactlyOnceWith(openFixture.params);
  expect((await readUiRecord(reviewCreateDraftAddress(scope))).value.request_id).toBe("open-review-1");
  restored.dispose();
});

it("restores action plans without changing selected versions, targets or source order", async () => {
  const plan = { ...mergeFixture.params, request_id: "keep-merge-plan", sources: [...mergeFixture.params.sources].reverse() };
  const draft = watchReviewActionDraft(scope, "merge", () => {});
  await draft.ready;
  await draft.write(plan);
  draft.dispose();
  const paint = vi.fn();
  const restored = watchReviewActionDraft(scope, "merge", paint);
  expect(await restored.ready).toEqual(plan);
  expect(paint).toHaveBeenCalledExactlyOnceWith(plan);
  expect((await readUiRecord(reviewActionDraftAddress(scope, "merge"))).value).toEqual(plan);
  restored.dispose();
});

it("scopes create and action drafts to their device, project, workspace, task and operation", async () => {
  const create = watchReviewCreateDraft(scope, () => {});
  const action = watchReviewActionDraft(scope, "merge", () => {});
  await Promise.all([create.ready, action.ready]);
  await create.write(openFixture.params);
  await action.write(mergeFixture.params);
  for (const altered of [{ deviceId: "other-device" }, { projectId: "other-project" }, { workspaceId: "other-workspace" }]) {
    expect(await readUiRecord(reviewCreateDraftAddress({ ...scope, ...altered }))).toBeUndefined();
  }
  for (const altered of [{ deviceId: "other-device" }, { projectId: "other-project" }, { taskId: "other-task" }, { snapshotId: "other-snapshot" }]) {
    expect(await readUiRecord(reviewActionDraftAddress({ ...scope, ...altered }, "merge"))).toBeUndefined();
  }
  expect(await readUiRecord(reviewActionDraftAddress(scope, "push"))).toBeUndefined();
  create.dispose(); action.dispose();
});

it("keeps drafts through reconnect metadata reads and never replays their writes on hydration", async () => {
  const create = watchReviewCreateDraft(scope, () => {});
  const action = watchReviewActionDraft(scope, "merge", () => {});
  await Promise.all([create.ready, action.ready]);
  await create.write(openFixture.params);
  await action.write(mergeFixture.params);
  create.dispose(); action.dispose();
  await rememberReviewSupport(scope.deviceId, { reviews: { get: true, pullRequests: true, open: true, merge: true } });
  const callRpc = vi.fn(async () => getFixture.examples[3].result);
  const repository = createTaskReviewRepository({ ...scope, callRpc });
  await repository.refresh();
  const restoredCreate = watchReviewCreateDraft(scope, () => {});
  const restoredAction = watchReviewActionDraft(scope, "merge", () => {});
  expect(await restoredCreate.ready).toEqual(openFixture.params);
  expect(await restoredAction.ready).toEqual(mergeFixture.params);
  expect(callRpc).toHaveBeenCalledExactlyOnceWith("tasks.review.get", { task_id: scope.taskId });
  restoredCreate.dispose(); restoredAction.dispose();
});

it("preserves the exact merge draft after stale_version metadata catch-up without retrying it", async () => {
  const draft = watchReviewActionDraft(scope, "merge", () => {});
  await draft.ready;
  await draft.write(mergeFixture.params);
  await rememberReviewSupport(scope.deviceId, { reviews: { get: true, pullRequests: true, merge: true } });
  const stale = Object.assign(new Error("Plan changed"), { code: "stale_version" });
  const callRpc = vi.fn(async (method) => {
    if (method === "tasks.review.get") return getFixture.examples[3].result;
    throw stale;
  });
  const repository = createTaskReviewRepository({ ...scope, callRpc });
  await expect(repository.mutate("merge", mergeFixture.params)).rejects.toBe(stale);
  expect(callRpc.mock.calls.map(([method]) => method)).toEqual(["tasks.review.merge", "tasks.review.get"]);
  expect((await readUiRecord(reviewActionDraftAddress(scope, "merge"))).value).toEqual(mergeFixture.params);
  draft.dispose();
});
