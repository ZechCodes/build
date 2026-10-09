// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const race = vi.hoisted(() => ({ writer: null, beforeWriteReturns: null }));
vi.mock("../src/core/taskReviewDrafts.js", async (importOriginal) => {
  const actual = await importOriginal();
  return { ...actual, watchReviewActionDraft(...args) {
    const writer = actual.watchReviewActionDraft(...args);
    race.writer = writer;
    return { ...writer, async write(value) {
      await writer.write(value);
      await race.beforeWriteReturns?.(value);
    } };
  } };
});

import { wipeCache } from "../src/core/localCache.js";
import { readUiRecord, wipeUiRecords, writeUiRecord } from "../src/core/localUiStore.js";
import { reviewActionDraftAddress } from "../src/core/taskReviewDrafts.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { writeReviewReply } from "../src/core/taskReviewCache.js";
import { openReviewPushForm } from "../src/core/workspaceReviewForm.js";
import opened from "../../fixtures/api/v1/tasks.review.open.json";
import pushed from "../../fixtures/api/v1/tasks.review.push.json";
import { called, holds, painted } from "./waits.js";

const scope = { deviceId: "push-race", projectId: "proj-1", workspaceId: "workspace-1", taskId: "task-1" };
const address = reviewActionDraftAddress(scope, "push");
const fact = { binding: opened.result.review.bindings[0], source: { directory: { name: "API" } },
  sync: { working_head: "3".repeat(40), received_head: "2".repeat(40) } };
const peerDraft = { expected_version: 7, sources: [{ directory_id: "dir-ui",
  expected_head: "9".repeat(40), expected_received_head: "8".repeat(40) }] };
let modal;

beforeEach(async () => {
  race.writer = null; race.beforeWriteReturns = null;
  await wipeCache(); await wipeUiRecords(); document.body.innerHTML = "";
  await rememberReviewSupport(scope.deviceId, { reviews: { get: true, pullRequests: true, push: true } });
  await writeReviewReply(scope, opened.result, 1);
});
afterEach(async () => { race.beforeWriteReturns = null; await modal?.close(); });

async function replaceWithPeerDraft() {
  await writeUiRecord(address, peerDraft);
  await race.writer.settled();
}

it.each(["during draft readback", "during request"])("preserves the peer's newer Push draft after success %s", async (timing) => {
  let settle;
  const reply = new Promise((resolve) => { settle = resolve; });
  const callRpc = vi.fn(() => reply);
  modal = await openReviewPushForm({ ...scope, callRpc }, opened.result.review, fact);
  if (timing === "during draft readback") race.beforeWriteReturns = async () => { await replaceWithPeerDraft(); };
  document.querySelector("[data-push-review-submit]").click();
  await called(callRpc, holds(() => expect(callRpc).toHaveBeenCalledOnce()));
  if (timing === "during request") await replaceWithPeerDraft();
  expect(callRpc.mock.calls[0]).toEqual(["tasks.review.push", { task_id: scope.taskId, expected_version: 1,
    sources: [{ directory_id: "dir-api", expected_head: fact.sync.working_head, expected_received_head: fact.sync.received_head }] }]);
  expect(document.querySelector("[data-push-review-pins]").textContent).toContain("Review version 1");
  expect(document.querySelector("[data-push-review-pins]").textContent).toContain(fact.sync.working_head);
  settle(pushed.result);
  await painted(holds(() => expect(document.querySelector("[role=dialog]")).toBeNull()));
  expect((await readUiRecord(address)).value).toEqual(peerDraft);
  modal = await openReviewPushForm({ ...scope, callRpc }, opened.result.review, fact);
  expect(document.querySelector("[data-push-review-pins]").textContent).toContain("Review version 7");
  expect(document.querySelector("[data-push-review-destinations]").textContent).toContain("build-review-ui");
  expect(callRpc).toHaveBeenCalledOnce();
});
