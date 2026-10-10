/** @vitest-environment jsdom */
// #408: PR status is a saved review fact, independent of board organization.
import { describe, expect, it } from "vitest";
import { taskRowHtml } from "../src/core/trackerListRender.js";
import { taskCardHtml } from "../src/core/trackerBoardRender.js";
import { taskHeadHtml } from "../src/core/trackerTaskRender.js";
import { inboxRowHtml } from "../src/core/inbox.js";
import { watchedTaskEntries } from "../src/core/watchedTaskRows.js";
import { columns, task, taskDetail } from "./trackerWireFixture.js";

const context = { columns: columns(), href: (one) => `#/tasks/${one.id}` };
const project = { id: "p1", deviceId: "dev-1", projectKey: "dev-1|p1", name: "Build" };
const parse = (html) => new DOMParser().parseFromString(html, "text/html");
const savedTask = (status, over = {}) => task({
  watched: true, assignee: { kind: "user" },
  review_summary: { task_id: "task-01K5Z1", workspace_id: "ws-1", version: 3, status, latest_published_snapshot_id: "snapshot-1" },
  ...over,
});
const watchedRow = (one, detail = null) => watchedTaskEntries([{
  project, tasks: [one], details: detail ? new Map([[one.id, detail]]) : new Map(),
}])[0];
const renderers = {
  list: (one) => taskRowHtml(one, context),
  board: (one) => taskCardHtml(one, context),
  head: taskHeadHtml,
  inbox: (one) => inboxRowHtml(watchedRow(one)),
};

describe.each(Object.entries(renderers))("cached PR summary on the %s", (_name, render) => {
  it.each([
    ["open", "Open"], ["approved", "Approved"], ["changes_requested", "Changes requested"],
    ["merged", "Merged"], ["closed", "Closed"],
  ])("shows the PR icon and saved %s status even when the board differs", (status, label) => {
    const doc = parse(render(savedTask(status, { status: "in_progress" })));
    const mark = doc.querySelector(".task-review-summary");
    expect(mark?.textContent.trim()).toBe(label);
    expect(mark?.querySelector("svg")).not.toBeNull();
    expect(mark?.getAttribute("aria-label")).toBe(`Pull request: ${label}`);
    expect(mark?.dataset.reviewStatus).toBe(status);
  });

  it("does not infer a PR from labels, task state or board column", () => {
    const one = task({ watched: true, assignee: { kind: "user" }, status: "in_review", labels: ["PR", "merged"] });
    expect(parse(render(one)).querySelector(".task-review-summary")).toBeNull();
  });
});

it("retains the exact cached list summary through the watched-task row seam", () => {
  const one = savedTask("changes_requested", { status: "in_progress" });
  const detail = taskDetail(savedTask("approved"));
  const entry = watchedRow(one, detail);
  expect(entry.reviewSummary).toBe(one.review_summary);
  expect(parse(inboxRowHtml(entry)).querySelector(".task-review-summary").textContent.trim()).toBe("Changes requested");
  expect(entry.facts).toBe("Assigned to you");
});

it("retains Closed on a task moved to Done without claiming a merge", () => {
  const one = savedTask("closed", { status: "done", state: "closed" });
  for (const render of [renderers.list, renderers.board, renderers.head]) {
    const mark = parse(render(one)).querySelector(".task-review-summary");
    expect(mark?.textContent.trim()).toBe("Closed");
    expect(mark?.textContent).not.toContain("Merged");
  }
});

it("keeps a PR summary on a quiet inbox row alongside its existing reason", () => {
  const entry = watchedRow(savedTask("approved"));
  const doc = parse(inboxRowHtml(entry, { quiet: true }));
  expect(doc.querySelector(".task-review-summary")?.textContent.trim()).toBe("Approved");
  expect(doc.body.textContent).toContain("Assigned to you");
});
