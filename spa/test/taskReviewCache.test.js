import { beforeEach, expect, it, vi } from "vitest";
import "fake-indexeddb/auto";
import { wipeCache, readCached } from "../src/core/localCache.js";
import { capabilitiesOf } from "../src/core/bridgeApi/v1/index.js";
import { rememberReviewSupport, readReviewSupport } from "../src/core/taskReviewSupport.js";
import { createTaskReviewRepository, reviewAddress, writeReviewRecord } from "../src/core/taskReviewCache.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";

const scope = { deviceId: "reviews-cache", projectId: "proj-1", taskId: "task-1" };
const support = { get: true, snapshot: true, diff: true, complete: true, act: true, comments: true };
const review = fixture.result.review;
beforeEach(async () => { await wipeCache(); });

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
  expect((await readCached(reviewAddress(scope))).value.review).toBeNull();
  await writeReviewRecord(scope, { ...review, version: 3 }, 100);
  await writeReviewRecord(scope, null, 200, 2);
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
