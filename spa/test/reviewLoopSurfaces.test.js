/** @vitest-environment jsdom */
import { describe, expect, it } from "vitest";
import { taskRowHtml } from "../src/core/trackerListRender.js";
import { taskCardHtml } from "../src/core/trackerBoardRender.js";
import { taskHeadHtml } from "../src/core/trackerTaskRender.js";
import { taskReviewSummaryHtml } from "../src/core/taskReviewSummaryRender.js";
import { inboxRowHtml } from "../src/core/inbox.js";
import { watchedTaskEntries } from "../src/core/watchedTaskRows.js";
import { taskAttention, watchedTaskReasons } from "../src/core/trackerAttentionModel.js";
import { columns, task } from "./trackerWireFixture.js";

const support = { reject: true, verdictDelivery: true };
const project = { id: "p1", deviceId: "dev-1", projectKey: "dev-1|p1", name: "Build" };
const context = { columns: columns(), href: () => "#/task", reviewSupport: support };
const one = (status = "rejected", fields = {}) => task({ watched: true, status: "in_review", assignee: { kind: "user" }, review_summary: { mode: "pull_request", status }, ...fields });
const entry = (value, extra = {}) => watchedTaskEntries([{ project, tasks: [value], reviewSupport: support, askedOnly: true, ...extra }])[0];
const renderers = {
  list: (value) => taskRowHtml(value, context), board: (value) => taskCardHtml(value, context),
  head: (value) => taskHeadHtml(value, context), inbox: (value) => inboxRowHtml(entry(value)),
};

describe.each(Object.entries(renderers))("review loop status on %s", (_name, render) => {
  it("paints Rejected from compact summary fields", () => {
    expect(render(one())).toContain('data-review-status="rejected"');
    expect(render(one())).toContain("Rejected");
  });
  it("labels explicit snapshot mode as Legacy review", () => {
    expect(render(one("open", { review_summary: { mode: "snapshot" } }))).toContain("Legacy review");
  });
});

it("recognizes the wire omission of snapshot mode on cached review metadata", () => {
  expect(taskReviewSummaryHtml({ snapshots: [] }, support)).toContain("Legacy review");
});

it("gates Rejected and Legacy labels on cached reject support", () => {
  expect(taskReviewSummaryHtml({ status: "rejected" })).toBe("");
  expect(taskReviewSummaryHtml({ mode: "snapshot" })).toBe("");
  expect(taskReviewSummaryHtml({ status: "approved" })).toContain("Approved");
});

const notice = (kind, overrides = {}) => ({ type: "comment", id: "tc-02", author: { kind: "build" }, review_notice: kind, mentions_user: true, ...overrides });
const detailed = (value, timeline) => ({ task: value, timeline });
const attention = (value, detail) => taskAttention(value, { detail, askedOnly: true, reviewSupport: support });

describe("bridge review-service attention mirror", () => {
  it.each(["revision_ready", "verdict_undelivered", "revision_undelivered"])("keeps marked %s asks on Done until read", (kind) => {
    const value = one("rejected", { status: "done", read_through: "te-01" });
    const detail = detailed(value, [notice(kind)]);
    expect(attention(value, detail).reasons).toEqual(["inbox_comment"]);
    expect(watchedTaskReasons(value, detail, true, support)).toEqual(["inbox_comment"]);
    expect(entry(value, { details: new Map([[value.id, detail]]) })).toMatchObject({ quiet: false, facts: "Mentioned you" });
    expect(attention({ ...value, read_through: "te-02" }, detail).reasons).toEqual([]);
    expect(entry({ ...value, read_through: "te-02" }, { details: new Map([[value.id, detail]]) })).toBeUndefined();
  });
  it("uses compact service counts without requiring task history", () => {
    const value = one("rejected", { status: "done", unread_service_mentions: 1 });
    expect(attention(value, null).reasons).toEqual(["inbox_comment"]);
    expect(entry(value)).toMatchObject({ quiet: false });
  });
  it("ignores ordinary Build mentions and unknown markers", () => {
    const value = one("rejected", { status: "done" });
    for (const row of [notice(null), notice("unknown"), notice("revision_ready", { mentions_user: false })]) {
      expect(attention(value, detailed(value, [row])).reasons).toEqual([]);
    }
  });
  it.each(["approved", "changes_requested"])("suppresses assignment alone for %s but keeps explicit asks", (status) => {
    const value = one(status);
    expect(attention(value, null).reasons).toEqual([]);
    expect(attention(value, detailed(value, [notice("revision_ready")])).reasons).toEqual(["inbox_comment"]);
  });
  it("keeps Open review and ordinary assignment attention", () => {
    expect(attention(one("open"), null).reasons).toEqual(["assigned_to_user"]);
    expect(attention(one("", { review_summary: undefined }), null).reasons).toEqual(["assigned_to_user"]);
  });
  it("requires cached capability support for service attention", () => {
    const value = one("rejected", { status: "done", unread_service_mentions: 1 });
    expect(taskAttention(value, { askedOnly: true }).reasons).toEqual([]);
  });
});
