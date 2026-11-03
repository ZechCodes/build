import { expect, it } from "vitest";
import { reviewHeadHtml } from "../src/core/taskReviewRender.js";

const scope = { deviceId: "device", projectId: "project" };
const staleReview = { state: "open", for_task_id: "original-work" };
const render = (parent) => reviewHeadHtml(staleReview, null, {}, {
  task: { id: "review", title: "Review", links: { parent_task_id: parent } }, scope,
});

it("links a reparented review header to its current work task", () => {
  expect(render("current-work")).toContain("/tasks/current-work");
  expect(render("current-work")).not.toContain("/tasks/original-work");
});

it.each([null, undefined])("does not revive the original work link after unlinking (%s)", (parent) => {
  expect(render(parent)).not.toContain("Work task");
});
