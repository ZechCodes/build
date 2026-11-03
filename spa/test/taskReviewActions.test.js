// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { mountTaskReviewActions } from "../src/core/taskReviewActions.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";

it("keeps legacy action results readable without forms, retry buttons or automatic completion", async () => {
  document.body.innerHTML = '<div id="actions"></div>';
  const review = { ...fixture.result.review, actions: [{ id: "old-action", snapshot_id: fixture.result.review.snapshots[0].id,
    directory_id: "dir-api", source_name: "API", status: "interrupted", steps: [{ kind: "push", branch: "main", status: "failed", error: "Offline" }] }] };
  const repository = { mutate: vi.fn() };
  const actions = mountTaskReviewActions(document.querySelector("#actions"), { review, snapshot: review.snapshots[0], repository, support: { act: true, complete: true } });
  expect(document.body.textContent).toContain("interrupted");
  expect(document.body.textContent).toContain("Offline");
  expect(document.querySelector("form, button, input, select")).toBeNull();
  expect(document.body.textContent).not.toContain("Merge and Push");
  actions.update({ ...review, state: "completed" });
  await Promise.resolve();
  expect(repository.mutate).not.toHaveBeenCalled();
  actions.dispose();
});
