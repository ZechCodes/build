// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import opened from "../../fixtures/api/v1/tasks.review.open.json";
import { called } from "./waits.js";

const owner = vi.hoisted(() => ({ session: "before" }));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => owner.session,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
const scope = { deviceId: "content-review-opening", projectId: "proj-1", workspaceId: "workspace-1" };
let modal;
beforeEach(async () => {
  vi.resetModules();
  owner.session = "before";
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = "";
  const { writeCached } = await import("../src/core/localCache.js");
  const { rememberReviewSupport } = await import("../src/core/taskReviewSupport.js");
  await writeCached({ deviceId: scope.deviceId, entityId: "", kind: "workspaces" }, [{
    id: scope.workspaceId, project_id: scope.projectId,
    directories: [{ id: "dir-api", source_id: "source-api", name: "API", is_git: true, base_branch: "main", branch: "build/work" }],
  }]);
  await rememberReviewSupport(scope.deviceId, { reviews: { get: true, pullRequests: true, open: true } });
});
afterEach(async () => { await modal?.close(); });

it("keeps a retired session review opening response from publishing a full task body", async () => {
  let resolve;
  const callRpc = vi.fn(() => new Promise((yes) => { resolve = yes; }));
  const { openReviewCreateForm } = await import("../src/core/workspaceReviewForm.js");
  const { readTaskRecord } = await import("../src/core/trackerCache.js");
  modal = await openReviewCreateForm({ ...scope, callRpc });
  const field = document.querySelector("[data-review-title]");
  field.value = "Open review";
  field.dispatchEvent(new Event("input", { bubbles: true }));
  const reviewer = document.querySelector("[data-assignee-select]");
  reviewer.value = "user";
  reviewer.dispatchEvent(new Event("change", { bubbles: true }));
  const submission = document.querySelector("[data-review-create-form]").onsubmit({ preventDefault() {} });
  await called(callRpc);
  owner.session = "after";
  const answer = structuredClone(opened.result);
  answer.task.project_id = scope.projectId;
  resolve(answer);
  await submission;
  expect(await readTaskRecord(scope.deviceId, scope.projectId, answer.task.id)).toBeNull();
  expect(callRpc).toHaveBeenCalledTimes(1);
});
