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
import { called, holds, painted } from "./waits.js";

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
  let refuse;
  const reply = new Promise((resolve, reject) => { settle = resolve; refuse = reject; });
  const callRpc = vi.fn(() => reply);
  modal = await openReviewCreateForm({ ...scope, callRpc });
  type("[data-review-title]", "Confirmed review");
  type("[data-review-description]", "Confirmed description");
  const select = document.querySelector("[data-assignee-select]");
  select.value = "user"; select.dispatchEvent(new Event("change", { bubbles: true }));
  return { callRpc, settle, refuse };
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
  await called(callRpc, holds(() => expect(callRpc).toHaveBeenCalledOnce()));
  expect(callRpc.mock.calls[0]).toEqual(["tasks.review.open", {
    workspace_id: scope.workspaceId, request_id: expect.any(String), title: "Confirmed review", description: "Confirmed description",
    reviewer: { kind: "user" }, bases: [{ directory_id: "dir-api", branch: "main" }], excluded_git_directory_ids: [],
  }]);
  expect(callRpc.mock.calls[0][1].request_id).not.toBe(peerRequest.request_id);
  expectConfirmedFields();
  settle(opened.result);
  await painted(holds(() => expect(document.querySelector("[role=dialog]")).toBeNull()));
  expect((await readUiRecord(address)).value).toEqual(peerDraft);
});

it("keeps a newer peer draft when the confirmed opening reports failed reviewer dispatch", async () => {
  const { callRpc, settle } = await mountPendingOpening();
  document.querySelector("[data-open-review-submit]").click();
  await called(callRpc, holds(() => expect(callRpc).toHaveBeenCalledOnce()));
  await replaceWithPeerDraft();
  settle({ ...opened.result, reviewer_dispatch: { state: "failed", error: "Reviewer unavailable" } });
  await painted(holds(() => expect(document.querySelector("[role=dialog]")).toBeNull()));
  expect((await readUiRecord(address)).value).toEqual(peerDraft);
});

it("freezes the confirmed caption and fields while its request is in flight", async () => {
  const { callRpc, settle } = await mountPendingOpening();
  document.querySelector("[data-open-review-submit]").click();
  expectConfirmedFields();
  await called(callRpc, holds(() => expect(callRpc).toHaveBeenCalledOnce()));
  await replaceWithPeerDraft();
  expectConfirmedFields();
  settle(opened.result);
  await painted(holds(() => expect(document.querySelector("[role=dialog]")).toBeNull()));
});

it("makes a definitively rejected opening editable and restores its durable fields on reopening", async () => {
  await writeCached({ deviceId: scope.deviceId, entityId: "", kind: "workspaces" }, [{
    id: scope.workspaceId, project_id: scope.projectId, directories: [
      { id: "dir-api", source_id: "source-api", name: "API", is_git: true, base_branch: "main", branch: "build/work" },
      { id: "dir-missing", source_id: "source-missing", name: "Missing", is_git: true, status: "unavailable", base_branch: "main" },
    ],
  }]);
  const { callRpc, refuse } = await mountPendingOpening();
  type('[data-review-base="dir-api"]', "bad base");
  document.querySelector("[data-open-review-submit]").click();
  await called(callRpc, holds(() => expect(callRpc).toHaveBeenCalledOnce()));
  refuse(Object.assign(new Error("branch must name a local Git branch"), { code: "invalid_params" }));
  await painted(holds(() => expect(document.querySelector("[data-review-form-error]").textContent).toContain("branch must name")));
  await painted(holds(() => expect(document.querySelector("[data-review-title]").disabled).toBe(false)));
  expect(document.querySelector("[data-review-description]").disabled).toBe(false);
  expect(document.querySelector("[data-assignee-select]").disabled).toBe(false);
  expect(document.querySelector('[data-review-base="dir-api"]').disabled).toBe(false);
  expect(document.querySelector('[data-review-base="dir-missing"]').disabled).toBe(true);
  expect(document.querySelector('[data-review-exclude="dir-missing"]').disabled).toBe(false);
  expect(document.querySelector("[data-open-review-submit]").textContent).toBe("Open review");
  const recovered = (await readUiRecord(address)).value;
  expect(recovered.title).toBe("Confirmed review");
  expect(recovered.bases[0].branch).toBe("bad base");
  expect(recovered.submitted).toBeUndefined();
  expect(callRpc).toHaveBeenCalledOnce();
  await modal.close();
  modal = await openReviewCreateForm({ ...scope, callRpc });
  expect(document.querySelector("[data-review-title]").value).toBe("Confirmed review");
  expect(document.querySelector("[data-review-title]").disabled).toBe(false);
  expect(document.querySelector('[data-review-base="dir-api"]').value).toBe("bad base");
  expect(document.querySelector('[data-review-base="dir-api"]').disabled).toBe(false);
  expect(document.querySelector('[data-review-base="dir-missing"]').disabled).toBe(true);
  expect(callRpc).toHaveBeenCalledOnce();
  type('[data-review-base="dir-api"]', "release");
  document.querySelector("[data-open-review-submit]").click();
  await called(callRpc, holds(() => expect(callRpc).toHaveBeenCalledTimes(2)));
  expect(callRpc.mock.calls[1][1].bases).toEqual([{ directory_id: "dir-api", branch: "release" }]);
});

it.each(["during draft readback", "during request"])("preserves a peer's newer draft after invalid_params %s", async (timing) => {
  const { callRpc, refuse } = await mountPendingOpening();
  if (timing === "during draft readback") race.beforeWriteReturns = async () => { await replaceWithPeerDraft(); };
  document.querySelector("[data-open-review-submit]").click();
  await called(callRpc, holds(() => expect(callRpc).toHaveBeenCalledOnce()));
  if (timing === "during request") await replaceWithPeerDraft();
  refuse(Object.assign(new Error("Rejected opening"), { error_code: "invalid_params" }));
  await painted(holds(() => expect(document.querySelector("[data-review-form-error]").textContent).toBe("Rejected opening")));
  await painted(holds(() => expect(document.querySelector("[data-review-title]").disabled).toBe(false)));
  expect((await readUiRecord(address)).value).toEqual(peerDraft);
  expect(callRpc).toHaveBeenCalledOnce();
});

it("keeps an uncertain transport failure pinned without resubmitting on reopening", async () => {
  const { callRpc, refuse } = await mountPendingOpening();
  document.querySelector("[data-open-review-submit]").click();
  await called(callRpc, holds(() => expect(callRpc).toHaveBeenCalledOnce()));
  refuse(Object.assign(new Error("Lost reply"), { uncertain: true }));
  await painted(holds(() => expect(document.querySelector("[data-review-form-error]").textContent).toBe("Lost reply")));
  expectConfirmedFields();
  expect((await readUiRecord(address)).value.submitted).toEqual(callRpc.mock.calls[0][1]);
  await modal.close();
  modal = await openReviewCreateForm({ ...scope, callRpc });
  expectConfirmedFields();
  expect(document.querySelector("[data-open-review-submit]").textContent).toBe("Retry opening");
  expect(callRpc).toHaveBeenCalledOnce();
});

it.each(["a".repeat(201), `${"é".repeat(100)}a`])("rejects a title over 200 UTF-8 bytes before sending", async (title) => {
  const { callRpc } = await mountPendingOpening();
  expect(document.querySelector("[data-review-title]").maxLength).toBe(200);
  type("[data-review-title]", title);
  document.querySelector("[data-open-review-submit]").click();
  await painted(holds(() => expect(document.querySelector("[data-review-form-error]").textContent).toContain("200 bytes")));
  expect(document.querySelector("[data-review-title]").disabled).toBe(false);
  expect(callRpc).not.toHaveBeenCalled();
  await modal.close();
  modal = await openReviewCreateForm({ ...scope, callRpc });
  expect(document.querySelector("[data-review-title]").value).toBe(title);
  expect((await readUiRecord(address)).value.submitted).toBeUndefined();
  expect(callRpc).not.toHaveBeenCalled();
});

it("accepts a title of exactly 200 UTF-8 bytes", async () => {
  const { callRpc, settle } = await mountPendingOpening();
  const title = "é".repeat(100);
  type("[data-review-title]", `  ${title}  `);
  document.querySelector("[data-open-review-submit]").click();
  await called(callRpc, holds(() => expect(callRpc).toHaveBeenCalledOnce()));
  expect(callRpc.mock.calls[0][1].title).toBe(title);
  settle(opened.result);
  await painted(holds(() => expect(document.querySelector("[role=dialog]")).toBeNull()));
});

it("unpins an older saved overlong opening after its definitive bridge rejection", async () => {
  const title = "a".repeat(201);
  const submitted = { ...peerRequest, title, reviewer: { kind: "user" }, bases: [{ directory_id: "dir-api", branch: "main" }] };
  await writeUiRecord(address, { ...peerDraft, title, submitted });
  const callRpc = vi.fn(async () => { throw Object.assign(new Error("title must be at most 200 bytes"), { code: "invalid_params" }); });
  modal = await openReviewCreateForm({ ...scope, callRpc });
  expect(document.querySelector("[data-review-title]").disabled).toBe(true);
  expect(callRpc).not.toHaveBeenCalled();
  document.querySelector("[data-open-review-submit]").click();
  await painted(holds(() => expect(document.querySelector("[data-review-title]").disabled).toBe(false)));
  expect(callRpc).toHaveBeenCalledExactlyOnceWith("tasks.review.open", submitted);
  expect(document.querySelector("[data-review-form-error]").textContent).toContain("200 bytes");
  expect((await readUiRecord(address)).value.submitted).toBeUndefined();
  type("[data-review-title]", "Corrected review");
  document.querySelector("[data-open-review-submit]").click();
  await called(callRpc, holds(() => expect(callRpc).toHaveBeenCalledTimes(2)));
  expect(callRpc.mock.calls[1][1].title).toBe("Corrected review");
});
