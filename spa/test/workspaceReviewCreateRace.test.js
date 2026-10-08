// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const race = vi.hoisted(() => ({ writer: null, beforeWriteReturns: null }));
vi.mock("../src/core/taskReviewDrafts.js", async (importOriginal) => {
  const actual = await importOriginal();
  return { ...actual, watchReviewCreateDraft(...args) {
    const writer = actual.watchReviewCreateDraft(...args);
    race.writer = writer;
    return { ...writer, async write(value) {
      await writer.write(value);
      await race.beforeWriteReturns?.(value);
    } };
  } };
});

import { wipeCache, writeCached } from "../src/core/localCache.js";
import { readUiRecord, wipeUiRecords, writeUiRecord } from "../src/core/localUiStore.js";
import { reviewCreateDraftAddress } from "../src/core/taskReviewDrafts.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { openReviewCreateForm } from "../src/core/workspaceReviewForm.js";
import opened from "../../fixtures/api/v1/tasks.review.open.json";

const scope = { deviceId: "create-race", projectId: "proj-1", workspaceId: "workspace-1" };
const address = reviewCreateDraftAddress(scope);
const peerRequest = { workspace_id: scope.workspaceId, request_id: "peer-opening", title: "Peer review",
  description: "Peer description", reviewer: { kind: "project_agent" },
  bases: [{ directory_id: "dir-api", branch: "release" }], excluded_git_directory_ids: [] };
const peerDraft = { request_id: peerRequest.request_id, title: peerRequest.title, description: peerRequest.description,
  reviewerDraft: { optionId: "project_agent" }, bases: peerRequest.bases, excluded_git_directory_ids: [], submitted: peerRequest };
let modal;

beforeEach(async () => {
  race.writer = null; race.beforeWriteReturns = null;
  await wipeCache(); await wipeUiRecords(); document.body.innerHTML = "";
  await writeCached({ deviceId: scope.deviceId, entityId: "", kind: "workspaces" }, [{
    id: scope.workspaceId, project_id: scope.projectId, directories: [
      { id: "dir-api", source_id: "source-api", name: "API", is_git: true, base_branch: "main", branch: "build/work" },
    ],
  }]);
  await rememberReviewSupport(scope.deviceId, { reviews: { get: true, pullRequests: true, open: true } });
});
afterEach(async () => { race.beforeWriteReturns = null; await modal?.close(); });

const type = (selector, value) => {
  const field = document.querySelector(selector);
  field.value = value; field.dispatchEvent(new Event("input", { bubbles: true }));
};
async function mountPendingOpening() {
  let settle;
  const reply = new Promise((resolve) => { settle = resolve; });
  const callRpc = vi.fn(() => reply);
  modal = await openReviewCreateForm({ ...scope, callRpc });
  type("[data-review-title]", "Confirmed review");
  type("[data-review-description]", "Confirmed description");
  const select = document.querySelector("[data-assignee-select]");
  select.value = "user"; select.dispatchEvent(new Event("change", { bubbles: true }));
  return { callRpc, settle };
}
async function replaceWithPeerDraft() {
  await writeUiRecord(address, peerDraft);
  await race.writer.settled();
}
function expectConfirmedFields() {
  expect(document.querySelector("[data-review-title]").value).toBe("Confirmed review");
  expect(document.querySelector("[data-review-description]").value).toBe("Confirmed description");
  expect(document.querySelector("[data-assignee-select]").value).toBe("user");
  expect(document.querySelector("[data-review-base]").value).toBe("main");
  expect(document.querySelector("[data-review-title]").disabled).toBe(true);
}

it("sends the confirmed opening when a peer replaces the draft during submission readback", async () => {
  const { callRpc, settle } = await mountPendingOpening();
  let replaced = false;
  race.beforeWriteReturns = async (value) => {
    if (!value?.submitted || replaced) return;
    replaced = true; await replaceWithPeerDraft();
  };
  document.querySelector("[data-open-review-submit]").click();
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledOnce());
  expect(callRpc.mock.calls[0]).toEqual(["tasks.review.open", {
    workspace_id: scope.workspaceId, request_id: expect.any(String), title: "Confirmed review", description: "Confirmed description",
    reviewer: { kind: "user" }, bases: [{ directory_id: "dir-api", branch: "main" }], excluded_git_directory_ids: [],
  }]);
  expect(callRpc.mock.calls[0][1].request_id).not.toBe(peerRequest.request_id);
  expectConfirmedFields();
  settle(opened.result);
  await vi.waitFor(() => expect(document.querySelector("[role=dialog]")).toBeNull());
  expect((await readUiRecord(address)).value).toEqual(peerDraft);
});

it("keeps a newer peer draft when the confirmed opening reports failed reviewer dispatch", async () => {
  const { callRpc, settle } = await mountPendingOpening();
  document.querySelector("[data-open-review-submit]").click();
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledOnce());
  await replaceWithPeerDraft();
  settle({ ...opened.result, reviewer_dispatch: { state: "failed", error: "Reviewer unavailable" } });
  await vi.waitFor(() => expect(document.querySelector("[role=dialog]")).toBeNull());
  expect((await readUiRecord(address)).value).toEqual(peerDraft);
});

it("freezes the confirmed caption and fields while its request is in flight", async () => {
  const { callRpc, settle } = await mountPendingOpening();
  document.querySelector("[data-open-review-submit]").click();
  expectConfirmedFields();
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledOnce());
  await replaceWithPeerDraft();
  expectConfirmedFields();
  settle(opened.result);
  await vi.waitFor(() => expect(document.querySelector("[role=dialog]")).toBeNull());
});
