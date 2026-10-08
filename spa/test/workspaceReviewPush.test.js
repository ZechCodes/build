// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { readUiRecord, wipeUiRecords } from "../src/core/localUiStore.js";
import { reviewActionDraftAddress, watchReviewActionDraft } from "../src/core/taskReviewDrafts.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { writeReviewReply } from "../src/core/taskReviewCache.js";
import { directoryCacheId } from "../src/core/directoryScope.js";
import { workspaceScope } from "../src/core/workspaceModel.js";
import { mountWorkspaceReviewEntry } from "../src/core/workspaceReviewEntry.js";
import opened from "../../fixtures/api/v1/tasks.review.open.json";
import pushed from "../../fixtures/api/v1/tasks.review.push.json";

const scope = { deviceId: "push-confirmation", projectId: "proj-1", workspaceId: "workspace-1" };
const actionScope = { ...scope, taskId: "task-1" };
const oid = (character) => character.repeat(40);
const source = (directoryId, head, received = oid("2")) => ({
  directory_id: directoryId, expected_head: head, expected_received_head: received,
});
const workspace = { id: scope.workspaceId, project_id: scope.projectId, name: "Work", directories: [
  { id: "dir-api", source_id: "source-api", name: "API", is_git: true, branch: "old-api" },
  { id: "dir-ui", source_id: "source-ui", name: "UI", is_git: true, branch: "old-ui" },
] };
const reviewReply = (version = 1, apiHead = oid("3"), received = oid("2")) => ({
  ...opened.result, review: { ...opened.result.review, version },
  sync: pushed.result.sync.map((sync) => ({ ...sync, revision: version + 10, health: "pending", pending_commits: 1,
    working_head: sync.directory_id === "dir-api" ? apiHead : oid("4"), received_head: received })),
});
let entry;
let writers;
beforeEach(async () => {
  await wipeCache(); await wipeUiRecords();
  writers = [];
  document.body.innerHTML = '<div id="entry"></div>';
  await writeCached({ deviceId: scope.deviceId, entityId: "", kind: "workspaces" }, [workspace]);
  await rememberReviewSupport(scope.deviceId, { reviews: { get: true, pullRequests: true, push: true } });
  for (const binding of opened.result.review.bindings) {
    const entityId = directoryCacheId(workspaceScope(scope.workspaceId, binding.source_id, workspace));
    await writeCached({ deviceId: scope.deviceId, entityId, kind: "refs" }, {
      current: { name: binding.dedicated_branch_ref.slice("refs/heads/".length), full_ref: binding.dedicated_branch_ref }, refs: [],
    });
  }
  await writeReviewReply(actionScope, reviewReply(), 1);
});
afterEach(async () => {
  document.querySelector("[data-review-form-cancel]")?.click();
  await vi.waitFor(() => expect(document.querySelector("[role=dialog]")).toBeNull());
  entry?.dispose();
  for (const writer of writers) writer.dispose();
});
const savedDraft = async () => (await readUiRecord(reviewActionDraftAddress(actionScope, "push")))?.value;
async function writeDraft(draft) {
  const writer = watchReviewActionDraft(actionScope, "push", () => {});
  writers.push(writer);
  await writer.ready;
  await writer.write(draft);
  return writer;
}
async function openPush(directoryId, callRpc) {
  entry = mountWorkspaceReviewEntry(document.querySelector("#entry"), { ...scope, callRpc });
  await vi.waitFor(() => expect(document.querySelector(`[data-review-push="${directoryId}"]`)).not.toBeNull());
  expect(callRpc).not.toHaveBeenCalled();
  document.querySelector(`[data-review-push="${directoryId}"]`).click();
  await vi.waitFor(() => expect(document.querySelector("[data-push-review-submit]")?.onclick).toBeTypeOf("function"));
}
const pins = () => document.querySelector("[data-push-review-pins]").textContent;
const dialog = () => document.querySelector("[role=dialog]").textContent;
const pushCalls = (callRpc) => callRpc.mock.calls.filter(([method]) => method === "tasks.review.push");

it("restores a saved API push when UI is clicked and confirms the saved source's exact destination", async () => {
  const draft = { expected_version: 1, sources: [source("dir-api", oid("3"))] };
  await writeDraft(draft);
  const callRpc = vi.fn(async () => { throw new Error("Unavailable"); });
  await openPush("dir-ui", callRpc);
  expect(dialog()).toContain("API");
  expect(dialog()).toContain("build-review-api refs/heads/review/1-api:refs/heads/review/1-api");
  expect(dialog()).not.toContain("build-review-ui");
  expect(pins()).toContain(oid("3"));
  expect(pins()).toContain(oid("2"));
  expect(await savedDraft()).toEqual(draft);
  expect(callRpc).not.toHaveBeenCalled();

  document.querySelector("[data-push-review-submit]").click();
  await vi.waitFor(() => expect(pushCalls(callRpc)).toHaveLength(1));
  expect(pushCalls(callRpc)[0]).toEqual(["tasks.review.push", { task_id: actionScope.taskId, ...draft }]);
});

it("repaints confirmation pins from another draft writer before submitting the exact updated request", async () => {
  const callRpc = vi.fn(async () => { throw new Error("Unavailable"); });
  await openPush("dir-api", callRpc);
  expect(pins()).toContain("Review version 1");
  const updated = { expected_version: 8, sources: [source("dir-api", oid("8"), oid("7"))] };
  await writeDraft(updated);
  await vi.waitFor(() => expect(pins()).toContain("Review version 8"));
  expect(pins()).toContain(oid("8"));
  expect(pins()).toContain(oid("7"));
  expect(pins()).not.toContain(oid("3"));
  expect(dialog()).toContain("build-review-api refs/heads/review/1-api:refs/heads/review/1-api");
  expect(callRpc).not.toHaveBeenCalled();

  document.querySelector("[data-push-review-submit]").click();
  await vi.waitFor(() => expect(pushCalls(callRpc)).toHaveLength(1));
  expect(pushCalls(callRpc)[0]).toEqual(["tasks.review.push", { task_id: actionScope.taskId, ...updated }]);
});

it("preserves a stale request until Use latest cached changes explicitly replaces its version and source pins", async () => {
  const initial = { expected_version: 1, sources: [source("dir-api", oid("3"))] };
  const stale = Object.assign(new Error("Review changed"), { code: "stale_version" });
  const callRpc = vi.fn(async (method) => {
    if (method === "tasks.review.get") return reviewReply(2, oid("5"), oid("3"));
    if (pushCalls(callRpc).length === 1) throw stale;
    throw new Error("Unavailable");
  });
  await openPush("dir-api", callRpc);
  document.querySelector("[data-push-review-submit]").click();
  await vi.waitFor(() => expect(document.querySelector("[data-review-form-error]").textContent).toContain("Review changed"));
  expect(callRpc.mock.calls.map(([method]) => method)).toEqual(["tasks.review.push", "tasks.review.get"]);
  expect(await savedDraft()).toEqual(initial);
  expect(pins()).toContain("Review version 1");
  expect(pins()).toContain(oid("3"));

  await writeReviewReply(actionScope, reviewReply(3, oid("6"), oid("5")), 100);
  expect(await savedDraft()).toEqual(initial);
  const reset = document.querySelector("[data-review-push-reset]");
  expect(reset).not.toBeNull();
  expect(reset.textContent).toBe("Use latest cached changes");
  reset.click();
  const latest = { expected_version: 3, sources: [source("dir-api", oid("6"), oid("5"))] };
  await vi.waitFor(async () => expect(await savedDraft()).toEqual(latest));
  expect(pins()).toContain("Review version 3");
  expect(pins()).toContain(oid("6"));
  expect(pins()).toContain(oid("5"));
  expect(pushCalls(callRpc)).toHaveLength(1);

  document.querySelector("[data-push-review-submit]").click();
  await vi.waitFor(() => expect(pushCalls(callRpc)).toHaveLength(2));
  expect(pushCalls(callRpc)[1]).toEqual(["tasks.review.push", { task_id: actionScope.taskId, ...latest }]);
});
