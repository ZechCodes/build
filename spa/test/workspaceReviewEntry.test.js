// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { writeReviewRecord } from "../src/core/taskReviewCache.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { mountWorkspaceReviewEntry, openWorkspaceReview } from "../src/core/workspaceReviewEntry.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";
import { painted } from "./waits.js";
let entry;
beforeEach(async () => { entry?.dispose(); await wipeCache(); document.body.innerHTML = '<div id="entry"></div>'; });
const options = { deviceId: "entry-device", projectId: "proj-1", workspaceId: "ws-1" };
it("shows an upgrade notice on snapshot-only bridges and never opens the removed modal", async () => {
  await rememberReviewSupport(options.deviceId, { reviews: { get: true, snapshot: true } });
  const callRpc = vi.fn();
  entry = mountWorkspaceReviewEntry(document.querySelector("#entry"), { ...options, callRpc });
  await painted(() => document.querySelector("#entry").textContent.includes("Update the bridge to request reviews"));
  expect(document.querySelector("[data-workspace-review]")).toBeNull();
  expect(await openWorkspaceReview({ ...options, callRpc })).toBeNull();
  expect(document.querySelector("[data-review-task]")).toBeNull();
  expect(callRpc).not.toHaveBeenCalled();
});
it("keeps saved legacy reviews readable with only the workspace creation path", async () => {
  await rememberReviewSupport(options.deviceId, { reviews: { get: true, snapshot: true, pullRequests: true, open: true } });
  await writeCached({ deviceId: options.deviceId, entityId: "", kind: "workspaces" }, [{ id: options.workspaceId, project_id: options.projectId, directories: [{ id: "api", is_git: true }] }]);
  await writeReviewRecord({ ...options, taskId: "task-1" }, { ...fixture.result.review, workspace_id: options.workspaceId }, 1);
  entry = mountWorkspaceReviewEntry(document.querySelector("#entry"), { ...options, callRpc: vi.fn() });
  await painted(() => document.querySelector("[data-workspace-review]"));
  expect(document.querySelector("[data-workspace-review]").textContent).toBe("Request review");
  document.querySelector("[data-workspace-review]").click();
  await painted(() => document.querySelector("[data-review-create-form]"));
  expect(document.querySelector("[data-review-task]")).toBeNull();
});
