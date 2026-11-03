// @vitest-environment jsdom
import { expect, it } from "vitest";
import { taskWorkReviewHtml } from "../src/core/taskWorkReview.js";

const context = { deviceId: "work-review-device", projectId: "p1", links: [], forTask: true };
const linked = { task_id: "review", number: 72, parent_task_id: "work", status: "changes_requested", version: 2 };
const work = { id: "work", project_id: "p1", links: { workspace_ids: ["ws1"] }, linked_reviews: [linked] };
const parse = (html) => new DOMParser().parseFromString(html, "text/html");

it("renders authoritative linked reviews from a cold task detail without other task caches", () => {
  const doc = parse(taskWorkReviewHtml(work, context));
  const chip = doc.querySelector("[data-work-task-review]");
  expect(chip.textContent).toBe("Review #72 · Changes requested");
  expect(chip.getAttribute("href")).toBe("#/device/work-review-device/project/p1/tasks/review");
});

it("offers the linked workspace Changes when the task has no saved review", () => {
  const links = [{ kind: "workspace", workspaceId: "ws1", label: "API work", route: {
    name: "workspace", deviceId: "work-review-device", projectId: "p1", workspaceId: "ws1", tab: "changes",
  } }];
  const doc = parse(taskWorkReviewHtml({ ...work, linked_reviews: [] }, { ...context, links }));
  expect(doc.body.textContent).toBe("Request a review from @API work");
  expect(doc.querySelector("a").getAttribute("href")).toBe("#/device/work-review-device/project/p1/workspace/ws1/changes");
});

it.each([{ ...context, forTask: false }, { ...context, review: {} }])("does not show work-review actions outside supported work tasks", (ctx) => {
  expect(taskWorkReviewHtml(work, ctx)).toBe("");
});

it("makes a bounded child list explicit", () => {
  const html = taskWorkReviewHtml({ ...work, linked_reviews_truncated: true }, context);
  expect(parse(html).body.textContent).toContain("Showing latest 1 reviews.");
});

it("bounds the saved projection defensively to the newest twenty reviews", () => {
  const linked_reviews = Array.from({ length: 25 }, (_, index) => ({ ...linked, task_id: `review-${index}`, number: index + 1 }));
  const doc = parse(taskWorkReviewHtml({ ...work, linked_reviews }, context));
  const chips = [...doc.querySelectorAll('[data-work-task-review]')];
  expect(chips).toHaveLength(20);
  expect(chips[0].textContent).toBe("Review #25 · Changes requested");
  expect(chips.at(-1).textContent).toBe("Review #6 · Changes requested");
  expect(doc.body.textContent).toContain("Showing latest 20 reviews.");
});

it("ignores stale child cache rows after the authoritative parent projection removes them", () => {
  const childReviews = [{ id: "review", number: 72, review_summary: linked }];
  expect(taskWorkReviewHtml({ ...work, linked_reviews: [] }, { ...context, childReviews })).toBe("");
});
