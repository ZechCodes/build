import { afterEach, beforeEach, expect, it, vi } from "vitest";
import "fake-indexeddb/auto";
import { wipeCache, readCached, writeCached } from "../src/core/localCache.js";
import * as cache from "../src/core/localCache.js";
import { forgetPushes, notePush } from "../src/core/pushFence.js";
import { capabilitiesOf } from "../src/core/bridgeApi/v1/index.js";
import { rememberReviewSupport, readReviewSupport } from "../src/core/taskReviewSupport.js";
import * as reviewSupport from "../src/core/taskReviewSupport.js";
import { replaceSessionList, sessionListObservation } from "../src/core/sessionListCache.js";
import {
  createTaskReviewRepository, readWorkspaceReview, refreshCachedReviews, refreshWorkspaceReview,
  reviewAddress, writeReviewRecord, writeReviewReply,
} from "../src/core/taskReviewCache.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";
import openFixture from "../../fixtures/api/v1/tasks.review.open.json";
import pushFixture from "../../fixtures/api/v1/tasks.review.push.json";
import mergeFixture from "../../fixtures/api/v1/tasks.review.merge.json";
import closeFixture from "../../fixtures/api/v1/tasks.review.close.json";

// These repositories use isolated injected callers; current-greeting dispatch
// is covered by taskReviewSupport's registered-device tests.
vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: () => null }));

const scope = { deviceId: "reviews-cache", projectId: "proj-1", taskId: "task-1" };
const support = { get: true, snapshot: true, diff: true, complete: true, act: true, comments: true,
  pullRequests: false, open: false, push: false, update: false, merge: false, close: false, reopen: false, refresh: false };
const review = fixture.result.review;
const pullRequest = fixture.examples[1].result;
const prSupport = Object.fromEntries(Object.keys(support).map((key) => [key, true]));
beforeEach(async () => { await wipeCache(); });
afterEach(() => { vi.restoreAllMocks(); forgetPushes(); });

it("gates each verb and comments independently and remembers the greeting", async () => {
  const caps = capabilitiesOf({ api_version: "3.6.0", capabilities: ["tasks.review.get", "tasks.review.diff"] });
  expect(caps.reviews).toEqual({ ...support, snapshot: false, complete: false, act: false, comments: false });
  await rememberReviewSupport(scope.deviceId, caps);
  expect(await readReviewSupport(scope.deviceId)).toEqual(caps.reviews);
  expect(await readReviewSupport("unknown")).toEqual(Object.fromEntries(Object.keys(support).map((key) => [key, false])));
});

it("never probes unsupported bridges", async () => {
  const callRpc = vi.fn();
  await createTaskReviewRepository({ ...scope, callRpc }).refresh();
  expect(callRpc).not.toHaveBeenCalled();
});

it("keeps a newer snapshot/completion against older replies from another tab", async () => {
  await writeReviewRecord(scope, { ...review, version: 3, state: "completed" }, 10);
  await writeReviewRecord(scope, { ...review, version: 2 }, 20);
  await writeReviewRecord(scope, null, 30);
  const cached = await readCached(reviewAddress(scope));
  expect(cached.value.review.version).toBe(3);
  expect(cached.value.review.state).toBe("completed");
  expect(reviewAddress(scope).entityId).toBe(scope.projectId);
});

it("stale mutations refetch but never retry the command or erase the caller's draft", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: support });
  const stale = Object.assign(new Error("Review changed"), { code: "stale_version" });
  const callRpc = vi.fn(async (method) => {
    if (method === "tasks.review.get") return { review: { ...review, version: 4 } };
    throw stale;
  });
  const repository = createTaskReviewRepository({ ...scope, callRpc });
  await expect(repository.mutate("snapshot", { expected_version: 1, workspace_id: "ws" })).rejects.toBe(stale);
  expect(callRpc.mock.calls.map(([method]) => method)).toEqual(["tasks.review.snapshot", "tasks.review.get"]);
  expect((await readCached(reviewAddress(scope))).value.review.version).toBe(4);
});

it("clears an authoritative missing review without letting an older null erase a concurrent snapshot", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: support });
  await writeReviewRecord(scope, review, 1);
  const repository = createTaskReviewRepository({ ...scope, callRpc: async () => ({ review: null }) });
  expect(await repository.refresh()).toBe(true);
  const cleared = (await readCached(reviewAddress(scope))).value;
  expect(cleared.review).toBeNull();
  await writeReviewRecord(scope, { ...review, version: 3 }, cleared.read_order + 1);
  await writeReviewRecord(scope, null, cleared.read_order + 2, 2);
  expect((await readCached(reviewAddress(scope))).value.review.version).toBe(3);
});

it("keeps cached review facts with a visible read failure, and ignores a superseded session", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: support });
  await writeReviewRecord(scope, review, 1);
  const repository = createTaskReviewRepository({ ...scope, callRpc: async () => { throw new Error("Source unavailable"); } });
  expect(await repository.refresh()).toBe(false);
  const cached = (await readCached(reviewAddress(scope))).value;
  expect(cached.review.version).toBe(1);
  expect(cached.error).toBe("Source unavailable");
  let active = true;
  const stale = createTaskReviewRepository({ ...scope, active: () => active, callRpc: async () => { active = false; return { review: null }; } });
  expect(await stale.refresh()).toBe(false);
  expect((await readCached(reviewAddress(scope))).value.review.version).toBe(1);
});

it("keeps complete PR metadata and additive open, publication and partial merge results", async () => {
  for (const answer of [openFixture.examples[0].result, pushFixture.examples[0].result, mergeFixture.examples[1].result]) {
    await writeReviewReply(scope, answer, answer.review.version);
    expect((await readCached(reviewAddress(scope))).value).toMatchObject(answer);
  }
  await writeReviewReply(scope, { review: mergeFixture.examples[1].result.review }, 100);
  const held = (await readCached(reviewAddress(scope))).value;
  expect(held.review.pull_request.status).toBe("merged");
  expect(held.merge_intents[0].state).toBe("failed");
  expect(held.sources[1].status).toBe("failed");
  expect(held.reviewer_dispatch.state).toBe("failed");
});

it("orders each observation independently when metadata is newer and its observations are older", async () => {
  const unavailable = fixture.examples[2].result;
  await writeReviewReply(scope, unavailable, 20);
  await writeReviewReply(scope, fixture.examples[3].result, 30);
  const held = (await readCached(reviewAddress(scope))).value;
  expect(held.review.version).toBe(7);
  expect(held.sync.find((row) => row.directory_id === "dir-api").revision).toBe(3);
  expect(held.sync.find((row) => row.directory_id === "dir-ui")).toEqual(unavailable.sync[1]);
});

it("keeps newer observations even when they come with stale metadata and replaces whole facts", async () => {
  await writeReviewReply(scope, fixture.examples[3].result, 20);
  const next = { ...pullRequest, sync: [{ ...pullRequest.sync[1], revision: 4, health: "unavailable", error: "Missing receiver" }] };
  await writeReviewReply(scope, next, 10);
  const afterFailure = (await readCached(reviewAddress(scope))).value;
  expect(afterFailure.review.version).toBe(7);
  expect(afterFailure.sync.find((row) => row.directory_id === "dir-ui").error).toBe("Missing receiver");
  await writeReviewReply(scope, { ...pullRequest, sync: [{ ...pullRequest.sync[1], revision: 5 }] }, 5);
  expect((await readCached(reviewAddress(scope))).value.sync.find((row) => row.directory_id === "dir-ui"))
    .toEqual({ ...pullRequest.sync[1], revision: 5 });
});

it("settles equal observation revisions by shared read order inside atomic writes", async () => {
  const newer = { ...pullRequest, sync: [{ ...pullRequest.sync[0], health: "pending", pending_commits: 2 }] };
  await Promise.all([writeReviewReply(scope, newer, 20), writeReviewReply(scope, pullRequest, 10)]);
  expect((await readCached(reviewAddress(scope))).value.sync[0].pending_commits).toBe(2);
});

it("refreshes discovered unopened reviews together with held metadata only once", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: prSupport });
  await writeReviewReply(scope, pullRequest, 1);
  const callRpc = vi.fn(async (_method, params) => ({ ...pullRequest, review: { ...pullRequest.review, task_id: params.task_id } }));
  await refreshCachedReviews({ ...scope, callRpc, discoveredTaskIds: ["new-task", "new-task", scope.taskId] });
  expect(callRpc.mock.calls.map(([, params]) => params.task_id)).toEqual([scope.taskId, "new-task"]);
  expect((await readCached(reviewAddress({ ...scope, taskId: "new-task" }))).value.review.task_id).toBe("new-task");
});

it("discovers workspace metadata before opening a task and retains facts on read failure", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: prSupport });
  const activeReview = { task_id: scope.taskId, workspace_id: "workspace-1", status: "open" };
  const workspaceScope = { ...scope, workspaceId: "workspace-1" };
  await writeCached({ deviceId: scope.deviceId, entityId: "", kind: "workspaces" }, [
    { id: "workspace-1", project_id: scope.projectId, active_review: activeReview },
  ]);
  const callRpc = vi.fn(async () => pullRequest);
  expect(await refreshWorkspaceReview({ ...workspaceScope, callRpc })).toBe(true);
  expect(await readWorkspaceReview(workspaceScope)).toMatchObject(pullRequest);
  expect(callRpc).toHaveBeenCalledExactlyOnceWith("tasks.review.get", { task_id: scope.taskId });
  expect(await refreshWorkspaceReview({ ...workspaceScope, callRpc: async () => { throw new Error("Source removed"); } })).toBe(false);
  expect(await readWorkspaceReview(workspaceScope)).toMatchObject({ ...pullRequest, error: "Source removed" });
});

it("does not read mismatched workspace hints or advertise PR support from only a verb", async () => {
  const callRpc = vi.fn();
  const workspaceScope = { ...scope, workspaceId: "workspace-1", callRpc };
  await rememberReviewSupport(scope.deviceId, { reviews: { ...prSupport, pullRequests: false } });
  expect(await refreshWorkspaceReview({ ...workspaceScope, activeReview: { task_id: scope.taskId, workspace_id: "workspace-1" } })).toBe(false);
  await rememberReviewSupport(scope.deviceId, { reviews: prSupport });
  expect(await refreshWorkspaceReview({ ...workspaceScope, activeReview: { task_id: scope.taskId, workspace_id: "elsewhere" } })).toBe(false);
  expect(callRpc).not.toHaveBeenCalled();
});

it("creates once with the exact open request and writes discovered task facts to cache", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: prSupport });
  const callRpc = vi.fn(async () => openFixture.result);
  const repository = createTaskReviewRepository({ deviceId: scope.deviceId, projectId: scope.projectId,
    workspaceId: "workspace-1", callRpc });
  await repository.mutate("open", openFixture.params);
  expect(callRpc).toHaveBeenCalledExactlyOnceWith("tasks.review.open", openFixture.params);
  expect((await readCached(reviewAddress(scope))).value).toMatchObject(openFixture.result);
});

it("gates PR and legacy operations against the cached review mode", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: prSupport });
  await writeReviewReply(scope, pullRequest, 1);
  const callRpc = vi.fn();
  const repository = createTaskReviewRepository({ ...scope, callRpc });
  for (const verb of ["act", "snapshot", "complete", "pullRequests"]) {
    await expect(repository.mutate(verb, { expected_version: 3 })).rejects.toThrow();
  }
  expect(callRpc).not.toHaveBeenCalled();
});

it("does not resurrect metadata or observations from a read preceding an authoritative missing review", async () => {
  await writeReviewReply(scope, pullRequest, 10);
  await writeReviewReply(scope, { review: null }, 30, pullRequest.review.version);
  await writeReviewReply(scope, { ...pullRequest, sync: [{ ...pullRequest.sync[0], revision: 100 }] }, 20);
  expect((await readCached(reviewAddress(scope))).value.review).toBeNull();
  expect((await readCached(reviewAddress(scope))).value.sync).toBeUndefined();
});

it("filters invalidations while discovering summaries only on feature-capable devices", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: prSupport });
  await writeReviewReply(scope, pullRequest, 1);
  const callRpc = vi.fn(async () => pullRequest);
  await refreshCachedReviews({ ...scope, callRpc, discoveredTaskIds: ["new-task"] }, [scope.taskId]);
  expect(callRpc).toHaveBeenCalledExactlyOnceWith("tasks.review.get", { task_id: scope.taskId });
  callRpc.mockClear();
  await rememberReviewSupport(scope.deviceId, { reviews: { ...prSupport, pullRequests: false } });
  await refreshCachedReviews({ ...scope, callRpc, discoveredTaskIds: ["new-task"] });
  expect(callRpc).toHaveBeenCalledExactlyOnceWith("tasks.review.get", { task_id: scope.taskId });
});

const delayReviewMerge = () => {
  const started = Promise.withResolvers();
  const continueMerge = Promise.withResolvers();
  const original = cache.mergeCachedAtomically;
  vi.spyOn(cache, "mergeCachedAtomically").mockImplementation(async (address, merge) => {
    started.resolve();
    await continueMerge.promise;
    return original(address, merge);
  });
  return { started: started.promise, release: continueMerge.resolve };
};

it.each(["read", "failure", "mutation"])("discards queued %s cache writes if the repository stops before its transaction", async (operation) => {
  await rememberReviewSupport(scope.deviceId, { reviews: prSupport });
  await writeReviewRecord(scope, review, 1);
  const held = await readCached(reviewAddress(scope));
  let active = true;
  const delayed = delayReviewMerge();
  const callRpc = async () => {
    if (operation === "failure") throw new Error("Source unavailable");
    return pullRequest;
  };
  const repository = createTaskReviewRepository({ ...scope, callRpc, active: () => active });
  const pending = operation === "mutation" ? repository.mutate("push", { expected_version: 1 }) : repository.refresh();
  await delayed.started;
  active = false;
  delayed.release();
  await pending;
  expect(await readCached(reviewAddress(scope))).toEqual(held);
});

it.each(["read", "failure", "mutation"])("discards queued %s cache writes if the project is removed before its transaction", async (operation) => {
  await rememberReviewSupport(scope.deviceId, { reviews: prSupport });
  await writeReviewRecord(scope, review, 1);
  const held = await readCached(reviewAddress(scope));
  const delayed = delayReviewMerge();
  const callRpc = async () => {
    if (operation === "failure") throw new Error("Source unavailable");
    return pullRequest;
  };
  const repository = createTaskReviewRepository({ ...scope, callRpc });
  const pending = operation === "mutation" ? repository.mutate("push", { expected_version: 1 }) : repository.refresh();
  await delayed.started;
  notePush(reviewAddress(scope), { removed: true });
  delayed.release();
  await pending;
  expect(await readCached(reviewAddress(scope))).toEqual(held);
});

it("returns an unchanged mutation answer only after its cache write settles", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: prSupport });
  const delayed = delayReviewMerge();
  const answer = pushFixture.examples[0].result;
  const repository = createTaskReviewRepository({ ...scope, callRpc: async () => answer });
  let settled = false;
  const pending = repository.mutate("push", pushFixture.params).then((result) => { settled = true; return result; });
  await delayed.started;
  expect(settled).toBe(false);
  expect(await readCached(reviewAddress(scope))).toBeUndefined();
  delayed.release();
  expect(await pending).toBe(answer);
  expect((await readCached(reviewAddress(scope))).value).toMatchObject(answer);
});

it("checks current greeting authority before sending a mutation advertised by older cached support", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: prSupport });
  const refusal = Object.assign(new Error("Current bridge does not support merge"), { code: "unknown_method" });
  const guardedRpc = vi.fn(async () => { throw refusal; });
  const factory = vi.spyOn(reviewSupport, "reviewMutationRpc").mockReturnValue(guardedRpc);
  const callRpc = vi.fn();
  const repository = createTaskReviewRepository({ ...scope, callRpc });
  await expect(repository.mutate("merge", mergeFixture.params)).rejects.toBe(refusal);
  expect(factory).toHaveBeenCalledExactlyOnceWith(scope.deviceId, callRpc);
  expect(guardedRpc).toHaveBeenCalledExactlyOnceWith("tasks.review.merge", mergeFixture.params);
  expect(callRpc).not.toHaveBeenCalled();
  expect(await readCached(reviewAddress(scope))).toBeUndefined();
});

const workspaceListAddress = { deviceId: scope.deviceId, entityId: "", kind: "workspaces" };
const workspaceScope = { ...scope, workspaceId: "workspace-1" };
async function clearWorkspaceReview() {
  const workspace = { id: workspaceScope.workspaceId, project_id: scope.projectId };
  await replaceSessionList(workspaceListAddress, "workspaces", [{ ...workspace, active_review: {
    task_id: scope.taskId, workspace_id: workspaceScope.workspaceId, version: 3, status: "open",
  } }]);
  const observation = await sessionListObservation(workspaceListAddress, "workspaces");
  await replaceSessionList(workspaceListAddress, "workspaces", [workspace], undefined, observation);
}

it("honors a canonical workspace summary clear when old task metadata remains cached", async () => {
  await writeReviewReply(scope, pullRequest, 10);
  await clearWorkspaceReview();
  expect((await readCached(workspaceListAddress)).value[0]).not.toHaveProperty("active_review");
  expect(await readWorkspaceReview(workspaceScope)).toBeNull();
});

it("finds a distinct newly opened PR before its workspace summary refreshes after a clear", async () => {
  await writeReviewReply(scope, pullRequest, 10);
  await clearWorkspaceReview();
  const opened = { ...openFixture.result, task: { ...openFixture.result.task, id: "new-pr-task" },
    review: { ...openFixture.result.review, task_id: "new-pr-task" } };
  await writeReviewReply({ ...scope, taskId: "new-pr-task" }, opened, 20);
  expect(await readWorkspaceReview(workspaceScope)).toMatchObject(opened);
});

it.each([
  ["closed", closeFixture.result],
  ["merged", mergeFixture.examples[1].result],
])("retains a cached %s PR link when a legacy workspace row has no summary authority", async (_status, reply) => {
  await replaceSessionList(workspaceListAddress, "workspaces", [{ id: workspaceScope.workspaceId, project_id: scope.projectId }]);
  await writeReviewReply(scope, reply, 10);
  expect(await readWorkspaceReview(workspaceScope)).toMatchObject(reply);
});
