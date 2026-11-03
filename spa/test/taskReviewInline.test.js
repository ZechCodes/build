// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { inlineReviewRowsHtml, paintInlineReviewRows } from "../src/core/taskReviewInline.js";

const anchor = { snapshot_id: "revision", directory_id: "directory", path: "src/a.js", side: "new", line: 2 };
const context = { snapshotId: "revision", directoryId: "directory", path: "src/a.js", side: "new", line: 2 };
const comment = (id, fields = {}) => ({ id, type: "comment", author: { kind: "user" }, body: "Please **keep** this.", anchor, ...fields });

describe("read-only review inline comments", () => {
  it("renders escaped markdown beneath the exact anchored line", () => {
    const host = document.createElement("tbody");
    host.innerHTML = inlineReviewRowsHtml([comment("one", { body: "**Keep** <img src=x onerror=alert(1)>" })], context);
    expect(host.querySelector("tr[data-review-inline]")).toBeTruthy();
    expect(host.querySelector("td").colSpan).toBe(3);
    expect(host.querySelector("strong").textContent).toBe("You");
    expect(host.querySelector(".markdown strong").textContent).toBe("Keep");
    expect(host.querySelector("img")).toBeNull();
    expect(host.textContent).toContain("<img src=x onerror=alert(1)>");
    expect(host.querySelector("textarea, button, input")).toBeNull();
  });

  it("adds Reply only to commentable threads", () => {
    const host = document.createElement('tbody');
    host.innerHTML = inlineReviewRowsHtml([comment('parent')], { ...context, replyable: true });
    expect(host.querySelector('[data-review-inline-reply]').dataset.reviewInlineReply).toBe('parent');
  });

  it("separates revisions, directories, paths, sides and commits", () => {
    for (const change of [{ snapshotId: "other" }, { directoryId: "other" }, { path: "other" }, { side: "old" }, { line: 3 }, { commit: "abc" }]) {
      expect(inlineReviewRowsHtml([comment("one")], { ...context, ...change })).toBe("");
    }
    const committed = comment("commit", { anchor: { ...anchor, commit: "abc" } });
    expect(inlineReviewRowsHtml([committed], context)).toBe("");
    expect(inlineReviewRowsHtml([committed], { ...context, commit: "abc" })).toContain("data-review-inline");
  });

  it("accepts normalized timeline rows and places their replies with the anchored parent", () => {
    const rows = [
      { key: "parent", actor: { kind: "user" }, body: "Parent", anchor, type: "comment" },
      { key: "reply", actor: { kind: "user" }, body: "Reply", replyTo: "parent", type: "comment" },
      { key: "event", body: "Not a comment", type: "event" },
    ];
    const html = inlineReviewRowsHtml(rows, context);
    expect(html).toContain("Parent");
    expect(html).toContain("Reply");
    expect(html).not.toContain("Not a comment");
  });

  it("adds source-line threads once across repaints", () => {
    const host = document.createElement("div");
    host.innerHTML = '<table><tbody><tr data-new-line="1"><td>first</td></tr><tr data-new-line="2"><td>second</td></tr></tbody></table>';
    paintInlineReviewRows(host, [comment("one")], { ...context, colspan: 2 });
    paintInlineReviewRows(host, [comment("one")], { ...context, colspan: 2 });
    const line = host.querySelector('tr[data-new-line="2"]');
    expect(line.nextElementSibling.matches("[data-review-inline]")).toBe(true);
    expect(host.querySelectorAll("[data-review-inline]")).toHaveLength(1);
    expect(line.nextElementSibling.querySelector("td").colSpan).toBe(2);
  });
});

it("marks non-deciding inline opinions as Advisory from cached support and assignment", () => {
  const rows = [comment("other", { author: { kind: "agent", agent_id: "other" }, opinion: { snapshot_id: "revision", verdict: "approve" } }), comment("assigned", { author: { kind: "agent", agent_id: "assigned" }, opinion: { snapshot_id: "revision", verdict: "approve" } })];
  const reading = { ...context, reviewSupport: { reject: true }, reviewAssignee: { kind: "agent", agent_id: "assigned" } };
  expect(inlineReviewRowsHtml(rows, reading).match(/Advisory/g)).toHaveLength(1);
  expect(inlineReviewRowsHtml(rows, context)).not.toContain("Advisory");
});
