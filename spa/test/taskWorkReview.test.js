// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, expect, it, vi } from "vitest";
import { taskWorkReviewHtml, watchWorkTaskReviews } from "../src/core/taskWorkReview.js";
import { writeTasksRecord, writeTaskRecord, taskRecord } from "../src/core/trackerCache.js";
import { wipeCache } from "../src/core/localCache.js";

const context = { deviceId: "work-review-device", projectId: "p1", links: [], forTask: true };
const work = { id: "work", project_id: "p1", links: { workspace_ids: ["ws1"] } };
const child = { id: "review", number: 72, links: { parent_task_id: "work" }, review_summary: {
  task_id: "review", for_task_id: "work", status: "changes_requested", version: 2,
} };
let stop;
afterEach(async () => { stop?.(); await wipeCache(); });
const parse = (html) => new DOMParser().parseFromString(html, "text/html");

it("renders child review chips and their saved status with review task links", () => {
  const doc = parse(taskWorkReviewHtml(work, { ...context, childReviews: [child] }));
  const chip = doc.querySelector("[data-work-task-review]");
  expect(chip.textContent).toBe("Review #72 · Changes requested");
  expect(chip.getAttribute("href")).toBe("#/device/work-review-device/project/p1/tasks/review");
});

it("renders authoritative linked reviews from a cold task detail without other task caches", () => {
  const doc = parse(taskWorkReviewHtml({ ...work, linked_reviews: [{ ...child.review_summary, number: 72 }] }, context));
  expect(doc.querySelector('[data-work-task-review]').textContent).toBe("Review #72 · Changes requested");
});

it("offers the linked workspace Changes when the task has no saved review", () => {
  const links = [{ kind: "workspace", workspaceId: "ws1", label: "API work", route: {
    name: "workspace", deviceId: "work-review-device", projectId: "p1", workspaceId: "ws1", tab: "changes",
  } }];
  const doc = parse(taskWorkReviewHtml(work, { ...context, links }));
  expect(doc.body.textContent).toBe("Request a review from @API work");
  expect(doc.querySelector("a").getAttribute("href")).toBe("#/device/work-review-device/project/p1/workspace/ws1/changes");
  expect(taskWorkReviewHtml({ ...work, review_summary: child.review_summary }, { ...context, links })).toBe("");
});

it("observes child review list and detail facts from the cache without a bridge", async () => {
  const seen = vi.fn();
  await writeTasksRecord(context.deviceId, context.projectId, { tasks: [child], columns: [] });
  stop = watchWorkTaskReviews({ ...context, taskId: "work", onChange: seen });
  await vi.waitFor(() => expect(seen).toHaveBeenCalledWith([expect.objectContaining(child)]));
  const updated = { ...child, review_summary: { ...child.review_summary, version: 3, status: "approved" } };
  await writeTaskRecord(context.deviceId, context.projectId, child.id, taskRecord(updated));
  await vi.waitFor(() => expect(seen).toHaveBeenLastCalledWith([expect.objectContaining(updated)]));
});

it("does not promise review requests on an older bridge", () => {
  expect(taskWorkReviewHtml({ ...work, linked_reviews: [{ ...child.review_summary, number: 72 }] }, { ...context, forTask: false })).toBe("");
});
it("makes a bounded child list explicit", () => {
  const html = taskWorkReviewHtml({ ...work, linked_reviews: [{ ...child.review_summary, number: 72 }], linked_reviews_truncated: true }, context);
  expect(parse(html).body.textContent).toContain("Showing latest 1 reviews.");
});

it("bounds the union of cached child rows and saved projections to the newest twenty reviews", () => {
  const childReviews = Array.from({ length: 25 }, (_, index) => ({ ...child, id: `review-${index}`, number: index + 1,
    review_summary: { ...child.review_summary, task_id: `review-${index}` } }));
  const doc = parse(taskWorkReviewHtml(work, { ...context, childReviews }));
  const chips = [...doc.querySelectorAll('[data-work-task-review]')];
  expect(chips).toHaveLength(20);
  expect(chips[0].textContent).toBe("Review #25 · Changes requested");
  expect(chips.at(-1).textContent).toBe("Review #6 · Changes requested");
  expect(doc.body.textContent).toContain("Showing latest 20 reviews.");
});

it("retains a saved review number when a newer cached summary lacks it", () => {
  const doc = parse(taskWorkReviewHtml({ ...work, linked_reviews: [{ ...child.review_summary, number: 72 }] }, {
    ...context, childReviews: [{ ...child, number: undefined, review_summary: { ...child.review_summary, version: 3, status: "approved" } }],
  }));
  expect(doc.querySelector('[data-work-task-review]').textContent).toBe("Review #72 · Approved");
});
