import { describe, expect, it } from "vitest";
import { timelineRows } from "../src/core/trackerTimeline.js";
import { timelineHtml, timelineParts } from "../src/core/trackerTaskRender.js";

const support = { reject: true, verdictDelivery: true };
const snapshots = [{ id: "r1", number: 1 }, { id: "r2", number: 2 }];
const context = { reviewSupport: support, reviewAssignee: { kind: "agent", agent_id: "assigned" }, reviewSnapshots: snapshots };
const opinion = (id, actor, snapshot = "r1") => ({ key: id, type: "comment", actor, body: "Opinion", opinion: { snapshot_id: snapshot, verdict: "approve" } });

describe("review loop timeline", () => {
  it("marks only a non-deciding agent's opinion as advisory", () => {
    const rows = [opinion("user", { kind: "user" }), opinion("assigned", { kind: "agent", agent_id: "assigned" }), opinion("other", { kind: "agent", agent_id: "other" })];
    const html = timelineHtml(rows, context);
    expect(html.match(/Advisory/g)).toHaveLength(1);
    expect(timelineHtml(rows, { ...context, reviewSupport: {} })).not.toContain("Advisory");
    expect(timelineHtml(rows, { ...context, reviewAssignee: { kind: "user" } }).match(/Advisory/g)).toHaveLength(2);
  });

  it("renders Reject opinions only with the reject capability", () => {
    const rows = [{ ...opinion("reject", { kind: "user" }), opinion: { snapshot_id: "r1", verdict: "reject" } }];
    expect(timelineHtml(rows, context)).toContain('data-verdict="reject">Rejected');
    expect(timelineHtml(rows, { ...context, reviewSupport: {} })).not.toContain('data-verdict="reject"');
  });

  it("adds keyed revision dividers without reordering old-revision replies", () => {
    const rows = [opinion("first", { kind: "user" }), { key: "event", type: "event", kind: "moved" }, opinion("second", { kind: "user" }, "r2"), opinion("old", { kind: "user" })];
    const parts = timelineParts(rows, context).rows;
    expect(parts.map((part) => part.name)).toEqual(["revision:r1:first", "first", "event", "revision:r2:second", "second", "revision:r1:old", "old"]);
    expect(parts[0].html).toContain("Revision 1");
    expect(parts[3].html).toContain("Revision 2");
    expect(timelineHtml(rows, { ...context, reviewSupport: {} })).not.toContain('class="task-review-revision-divider"');
  });

  it("keeps replies in their original revision while ordinary events follow publication time", () => {
    const reviewSnapshots = [{ id: "r1", number: 1, created_at: "2026-11-01" }, { id: "r2", number: 2, created_at: "2026-11-02" }];
    const rows = [opinion("old", { kind: "user" }), { key: "event", type: "event", kind: "moved", at: "2026-11-02T01:00:00Z" }, { key: "reply", type: "comment", actor: { kind: "user" }, body: "Late reply", replyTo: "old", at: "2026-11-03T01:00:00Z" }];
    const names = timelineParts(rows, { ...context, reviewSnapshots }).rows.map((part) => part.name);
    expect(names).toEqual(["revision:r1:old", "old", "revision:r2:event", "event", "revision:r1:reply", "reply"]);
  });

  it("normalizes and explains marked Build notices with delivery support", () => {
    const rows = timelineRows(["revision_ready", "verdict_undelivered", "revision_undelivered"].map((kind) => ({ id: kind, type: "comment", author: { kind: "build" }, review_notice: kind, body: "Plain error" })));
    expect(rows.map((row) => row.reviewNotice)).toEqual(["revision_ready", "verdict_undelivered", "revision_undelivered"]);
    const html = timelineHtml(rows, context);
    expect(html).toContain("A new revision is ready for review.");
    expect(html).toContain("The verdict could not be delivered to the author.");
    expect(html).toContain("The revision could not be delivered to the reviewer.");
    expect(html).toContain("Plain error");
    expect(timelineHtml(rows, { ...context, reviewSupport: {} })).not.toContain("could not be delivered");
    expect(timelineHtml([{ ...rows[1], actor: { kind: "agent", agent_id: "other" } }], context)).not.toContain("could not be delivered");
  });
});
