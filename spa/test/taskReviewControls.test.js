// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { mountTaskReviewControls } from "../src/core/taskReviewControls.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";

let controls;
const review = fixture.result.review;
const options = { deviceId: "control-device", projectId: "proj-1", taskId: "task-1", review,
  snapshot: review.snapshots[0], support: { snapshot: true, complete: true }, workspaces: [] };
const mount = (repository, over = {}) => {
  document.body.innerHTML = '<div id="controls"></div>';
  controls = mountTaskReviewControls(document.querySelector('#controls'), { ...options, repository, ...over });
};
beforeEach(async () => { controls?.dispose(); await wipeUiRecords(); });

it("sends one snapshot with exact per-directory base overrides and preserves a stale draft", async () => {
  let reject;
  const repository = { mutate: vi.fn(() => new Promise((_, fail) => { reject = fail; })) };
  mount(repository);
  const field = document.querySelector('[data-review-base="dir-api"]');
  field.value = "release/candidate"; field.dispatchEvent(new Event("input"));
  const form = document.querySelector('[data-review-save]');
  form.dispatchEvent(new Event("submit", { cancelable: true }));
  form.dispatchEvent(new Event("submit", { cancelable: true }));
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledTimes(1));
  expect(repository.mutate).toHaveBeenCalledWith("snapshot", { workspace_id: "workspace-1", expected_version: 1, base_overrides: { "dir-api": "release/candidate" } });
  reject(Object.assign(new Error("Review changed; refresh and try again"), { code: "stale_version" }));
  await vi.waitFor(() => expect(document.querySelector('[data-review-action-error]').textContent).toContain("Review changed"));
  controls.dispose(); mount(repository);
  await vi.waitFor(() => expect(document.querySelector('[data-review-base="dir-api"]').value).toBe("release/candidate"));
});

it("does not send old directory overrides when replacing the review workspace", async () => {
  const repository = { mutate: vi.fn(async () => {}) };
  mount(repository, { workspaces: [{ id: "workspace-2", name: "Other source" }] });
  const field = document.querySelector('[data-review-base="dir-api"]');
  field.value = "topic"; field.dispatchEvent(new Event("input"));
  const workspace = document.querySelector('[data-review-workspace]');
  workspace.value = "workspace-2"; workspace.dispatchEvent(new Event("change"));
  document.querySelector('[data-review-save]').dispatchEvent(new Event("submit", { cancelable: true }));
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("snapshot", { workspace_id: "workspace-2", base_overrides: {}, expected_version: 1 }));
});

it("completes with the user's description only and leaves completed review history readable", async () => {
  const repository = { mutate: vi.fn(async () => {}) };
  mount(repository);
  const field = document.querySelector('[data-review-description]');
  field.value = "  merged API to dev  "; field.dispatchEvent(new Event("input"));
  document.querySelector('[data-review-complete]').dispatchEvent(new Event("submit", { cancelable: true }));
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("complete", { expected_version: 1, description: "merged API to dev" }));
  controls.dispose(); mount(repository, { review: { ...review, state: "completed" } });
  expect(document.querySelector('[data-review-complete]')).toBeNull();
});
