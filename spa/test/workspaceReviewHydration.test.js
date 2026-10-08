// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const hydration = vi.hoisted(() => ({ gate: null, started: null }));
vi.mock("../src/core/taskReviewDrafts.js", async (importOriginal) => {
  const actual = await importOriginal();
  return { ...actual, watchReviewCreateDraft(...args) {
    const writer = actual.watchReviewCreateDraft(...args);
    return { ...writer, ready: writer.ready.then(async (saved) => {
      hydration.started.resolve();
      await hydration.gate.promise;
      return saved;
    }) };
  } };
});

import { wipeCache, writeCached } from "../src/core/localCache.js";
import { wipeUiRecords, writeUiRecord } from "../src/core/localUiStore.js";
import { reviewCreateDraftAddress } from "../src/core/taskReviewDrafts.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { mountWorkspaceReviewEntry } from "../src/core/workspaceReviewEntry.js";

const scope = { deviceId: "hydrating-review", projectId: "proj-1", workspaceId: "workspace-1" };
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
const request = { request_id: "saved-opening", workspace_id: scope.workspaceId, title: "Saved review", description: "Saved description",
  reviewer: { kind: "user" }, bases: [{ directory_id: "dir-api", branch: "release" }], excluded_git_directory_ids: [] };
const savedDraft = { request_id: request.request_id, title: request.title, description: request.description,
  reviewerDraft: { optionId: "user" }, bases: request.bases, excluded_git_directory_ids: [], submitted: request };
let entry;
let opening;

beforeEach(async () => {
  hydration.gate = deferred(); hydration.started = deferred(); opening = null;
  await wipeCache(); await wipeUiRecords();
  document.body.innerHTML = '<div id="entry"></div>';
  await writeCached({ deviceId: scope.deviceId, entityId: "", kind: "workspaces" }, [{
    id: scope.workspaceId, project_id: scope.projectId, directories: [
      { id: "dir-api", source_id: "source-api", name: "API", is_git: true, base_branch: "main", branch: "build/work" },
    ],
  }]);
  await rememberReviewSupport(scope.deviceId, { reviews: { get: true, pullRequests: true, open: true } });
});
afterEach(async () => {
  hydration.gate.resolve();
  const modal = await opening;
  await modal?.close();
  await entry?.dispose();
});

async function beginOpening() {
  const callRpc = vi.fn(async () => { throw new Error("Unavailable"); });
  entry = mountWorkspaceReviewEntry(document.querySelector("#entry"), { ...scope, callRpc });
  await vi.waitFor(() => expect(document.querySelector("[data-workspace-review]")?.onclick).toBeTypeOf("function"));
  opening = document.querySelector("[data-workspace-review]").onclick();
  await hydration.started.promise;
  return callRpc;
}
function expectHydrating() {
  document.querySelectorAll('[data-review-create-form] input, [data-review-create-form] textarea, [data-review-create-form] select')
    .forEach((field) => expect(field.disabled).toBe(true));
  expect(document.querySelector("[data-open-review-submit]").disabled).toBe(true);
  expect(document.querySelector("[data-review-form-cancel]").disabled).toBe(false);
  expect(document.querySelector("[data-review-form-cancel]").onclick).toBeTypeOf("function");
}

it.each(["fresh", "saved editable"])("keeps a %s real-mounted form disabled until draft hydration and handlers are ready", async (kind) => {
  if (kind === "saved editable") {
    const editable = { ...savedDraft };
    delete editable.submitted;
    await writeUiRecord(reviewCreateDraftAddress(scope), editable);
  }
  const callRpc = await beginOpening();
  expectHydrating();
  document.querySelector("[data-open-review-submit]").click();
  expect(callRpc).not.toHaveBeenCalled();
  hydration.gate.resolve();
  await opening;
  const title = document.querySelector("[data-review-title]");
  expect(title.disabled).toBe(false);
  expect(document.activeElement).toBe(title);
  expect(title.oninput).toBeTypeOf("function");
  expect(document.querySelector("[data-review-create-form]").onsubmit).toBeTypeOf("function");
  expect(document.querySelector("[data-open-review-submit]").disabled).toBe(false);
  title.value = "New review"; title.dispatchEvent(new Event("input", { bubbles: true }));
  document.querySelector("[data-open-review-submit]").click();
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledOnce());
  expect(callRpc.mock.calls[0][1].title).toBe("New review");
});

it("keeps restored submitted fields pinned and enables Retry only after its handler exists", async () => {
  await writeUiRecord(reviewCreateDraftAddress(scope), savedDraft);
  const callRpc = await beginOpening();
  expectHydrating();
  expect(document.querySelector("[data-review-title]").value).toBe(request.title);
  expect(callRpc).not.toHaveBeenCalled();
  hydration.gate.resolve();
  await opening;
  expect(document.querySelector("[data-review-title]").disabled).toBe(true);
  expect(document.querySelector("[data-review-base]").value).toBe("release");
  expect(document.querySelector("[data-review-create-form]").onsubmit).toBeTypeOf("function");
  expect(document.querySelector("[data-open-review-submit]").disabled).toBe(false);
  expect(document.querySelector("[data-open-review-submit]").textContent).toBe("Retry opening");
  expect(callRpc).not.toHaveBeenCalled();
  document.querySelector("[data-open-review-submit]").click();
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledExactlyOnceWith("tasks.review.open", request));
});

it("allows Cancel while the real draft readiness promise is pending", async () => {
  const callRpc = await beginOpening();
  const title = document.querySelector("[data-review-title]");
  document.querySelector("[data-review-form-cancel]").click();
  // Release readiness while dismissal can still be animating.
  hydration.gate.resolve();
  await opening;
  expect(title.disabled).toBe(true);
  expect(document.activeElement).not.toBe(title);
  await vi.waitFor(() => expect(document.querySelector("[role=dialog]")).toBeNull());
  expect(document.querySelector("[role=dialog]")).toBeNull();
  expect(callRpc).not.toHaveBeenCalled();
});
