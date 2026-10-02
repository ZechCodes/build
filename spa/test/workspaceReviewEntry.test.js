// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import { wipeCache } from "../src/core/localCache.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { writeTasksRecord } from "../src/core/trackerCache.js";
import { mountWorkspaceReviewEntry } from "../src/core/workspaceReviewEntry.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";

let entry;
beforeEach(async () => { entry?.dispose(); await wipeCache(); document.body.innerHTML = '<div id="entry"></div>'; });
const options = { deviceId: "entry-device", projectId: "proj-1", workspaceId: "ws-1" };
it("offers snapshot creation only when the cached bridge supports it", async () => {
  const callRpc = vi.fn();
  entry = mountWorkspaceReviewEntry(document.querySelector('#entry'), { ...options, callRpc });
  await new Promise((resolve) => setTimeout(resolve, 30));
  expect(document.querySelector('[data-workspace-review]')).toBeNull();
  expect(callRpc).not.toHaveBeenCalled();
  await rememberReviewSupport(options.deviceId, { reviews: { get: true, snapshot: true } });
  await vi.waitFor(() => expect(document.querySelector('[data-workspace-review]')).not.toBeNull());
});
it("saves all directories on the chosen task using its current review version, then opens the task", async () => {
  await rememberReviewSupport(options.deviceId, { reviews: { get: true, snapshot: true } });
  await writeTasksRecord(options.deviceId, options.projectId, { tasks: [{ id: "task-1", number: 42, title: "Review work", links: { workspace_ids: ["ws-1"] } }] });
  const callRpc = vi.fn(async () => fixture.result);
  const navigate = vi.fn();
  entry = mountWorkspaceReviewEntry(document.querySelector('#entry'), { ...options, callRpc, navigate });
  await vi.waitFor(() => expect(document.querySelector('[data-workspace-review]')).not.toBeNull());
  document.querySelector('[data-workspace-review]').click();
  await vi.waitFor(() => expect(document.querySelector('[data-review-task]')).not.toBeNull());
  document.querySelector('[data-save-workspace-review]').click();
  await vi.waitFor(() => expect(navigate).toHaveBeenCalled());
  expect(callRpc).toHaveBeenCalledWith("tasks.review.snapshot", { task_id: "task-1", workspace_id: "ws-1", expected_version: 1 });
  expect(navigate).toHaveBeenCalledWith({ name: "trackerTask", projectId: "proj-1", deviceId: "entry-device", taskId: "task-1" });
});

it("keeps the chosen task and shows a failed read without attempting a snapshot", async () => {
  await rememberReviewSupport(options.deviceId, { reviews: { get: true, snapshot: true } });
  await writeTasksRecord(options.deviceId, options.projectId, { tasks: [{ id: "task-1", number: 42, title: "Work" }] });
  const callRpc = vi.fn(async () => { throw new Error("Machine unavailable"); });
  entry = mountWorkspaceReviewEntry(document.querySelector('#entry'), { ...options, callRpc });
  await vi.waitFor(() => expect(document.querySelector('[data-workspace-review]')).not.toBeNull());
  document.querySelector('[data-workspace-review]').click();
  await vi.waitFor(() => expect(document.querySelector('[data-review-task]')).not.toBeNull());
  document.querySelector('[data-save-workspace-review]').click();
  await vi.waitFor(() => expect(document.querySelector('[data-workspace-review-error]').textContent).toContain("Machine unavailable"));
  expect(callRpc.mock.calls.map(([method]) => method)).toEqual(["tasks.review.get"]);
  expect(document.querySelector('[data-review-task]').value).toBe("task-1");
});
