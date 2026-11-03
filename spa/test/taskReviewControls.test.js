// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { mountTaskReviewControls } from "../src/core/taskReviewControls.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";
import prFixture from "../../fixtures/api/v1/tasks.review.open.json";

let controls;
const review = fixture.result.review;
const options = { deviceId: "control-device", projectId: "proj-1", taskId: "task-1", review,
  snapshot: review.snapshots[0], support: { snapshot: true, complete: true }, workspaces: [] };
const mount = (repository, over = {}) => {
  document.body.innerHTML = '<div id="controls"></div>';
  controls = mountTaskReviewControls(document.querySelector('#controls'), { ...options, repository, ...over });
};
beforeEach(async () => { controls?.dispose(); await wipeUiRecords(); });

it("renders cached PR status read-only even when legacy mutations are advertised", () => {
  const repository = { mutate: vi.fn() };
  mount(repository, { review: prFixture.result.review, support: { snapshot: true, complete: true, act: true } });
  expect(document.querySelector('[data-review-read-only]').textContent).toContain("Open");
  expect(document.querySelector('[data-review-read-only]').textContent).toContain("Read-only");
  expect(document.querySelector('[data-review-save]')).toBeNull();
  expect(document.querySelector('[data-review-complete]')).toBeNull();
  expect(repository.mutate).not.toHaveBeenCalled();

  controls.update({ ...prFixture.result.review, pull_request: { ...prFixture.result.review.pull_request, status: "changes_requested" } }, []);
  expect(document.querySelector('[data-review-read-only]').textContent).toContain("Changes requested");
});

it.each([null, { ...review, mode: "snapshot" }])("never offers task-side snapshot creation or updates: %j", (saved) => {
  const repository = { mutate: vi.fn() };
  mount(repository, { review: saved, support: { snapshot: true, complete: true, pullRequests: true, open: true } });
  expect(document.querySelector('[data-review-save]')).toBeNull();
  expect(document.querySelector('[data-review-snapshot-form]')).toBeNull();
  expect(document.querySelector('[data-review-workspace]')).toBeNull();
  expect(Boolean(document.querySelector('[data-review-close-legacy]'))).toBe(Boolean(saved));
  expect(repository.mutate).not.toHaveBeenCalled();
});

it("closes a legacy review explicitly without a completion form", async () => {
  const repository = { mutate: vi.fn(async () => {}) };
  mount(repository);
  expect(document.querySelector("form, textarea")).toBeNull();
  document.querySelector('[data-review-close-legacy]').click();
  document.querySelector('[data-confirm-ok]').click();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("complete", { expected_version: 1, description: "Closed legacy review" }));
  controls.dispose(); mount(repository, { review: { ...review, state: "completed" } });
  expect(document.querySelector('[data-review-close-legacy]')).toBeNull();
});
