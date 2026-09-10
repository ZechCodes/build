import { describe, it, expect } from "vitest";
import { addNotice, dismissNotice, noticeHtml } from "../src/core/notify.js";

describe("addNotice", () => {
  it("appends a notice with count 1 and a unique id, without mutating the input", () => {
    const before = [];
    const after = addNotice(before, { kind: "error", summary: "merge failed", detail: "conflict in a.js" });
    expect(before).toHaveLength(0);
    expect(after).toHaveLength(1);
    expect(after[0]).toMatchObject({ kind: "error", summary: "merge failed", detail: "conflict in a.js", count: 1 });
    expect(after[0].id).toBeDefined();
    const two = addNotice(after, { kind: "error", summary: "push failed" });
    expect(two[1].id).not.toBe(two[0].id);
  });

  it("defaults detail to empty string", () => {
    const list = addNotice([], { kind: "success", summary: "merged" });
    expect(list[0].detail).toBe("");
  });

  it("dedupes identical kind+summary+detail by incrementing count, without mutating", () => {
    const one = addNotice([], { kind: "error", summary: "rpc failed", detail: "timeout" });
    const two = addNotice(one, { kind: "error", summary: "rpc failed", detail: "timeout" });
    expect(two).toHaveLength(1);
    expect(two[0].count).toBe(2);
    expect(one[0].count).toBe(1);
    expect(two[0].id).toBe(one[0].id);
  });

  it("does not dedupe when kind, summary, or detail differ", () => {
    let list = addNotice([], { kind: "error", summary: "s", detail: "d" });
    list = addNotice(list, { kind: "success", summary: "s", detail: "d" });
    list = addNotice(list, { kind: "error", summary: "s2", detail: "d" });
    list = addNotice(list, { kind: "error", summary: "s", detail: "d2" });
    expect(list).toHaveLength(4);
  });
});

describe("dismissNotice", () => {
  it("removes the notice with the given id, without mutating", () => {
    const one = addNotice([], { kind: "error", summary: "a" });
    const two = addNotice(one, { kind: "error", summary: "b" });
    const after = dismissNotice(two, two[0].id);
    expect(two).toHaveLength(2);
    expect(after).toHaveLength(1);
    expect(after[0].summary).toBe("b");
  });

  it("returns an equal list when the id is unknown", () => {
    const one = addNotice([], { kind: "error", summary: "a" });
    expect(dismissNotice(one, -999)).toHaveLength(1);
  });
});

describe("noticeHtml", () => {
  const notice = (extra) => ({ id: 7, kind: "error", summary: "merge failed", detail: "", count: 1, ...extra });

  it("renders kind class, data-notice id, and role alert for errors", () => {
    const html = noticeHtml(notice());
    expect(html).toContain('class="notice error"');
    expect(html).toContain('data-notice="7"');
    expect(html).toContain('role="alert"');
    expect(html).toContain('<span class="notice-summary">merge failed</span>');
    expect(html).toContain('aria-label="Dismiss"');
  });

  it("uses role status for successes", () => {
    expect(noticeHtml(notice({ kind: "success" }))).toContain('role="status"');
    expect(noticeHtml(notice({ kind: "success" }))).toContain('class="notice success"');
  });

  it("renders the detail pre hidden with an expand button when detail is present", () => {
    const html = noticeHtml(notice({ detail: "stack trace here" }));
    expect(html).toContain('class="notice-expand"');
    expect(html).toContain('aria-label="Show details"');
    expect(html).toMatch(/<pre class="notice-detail" hidden>stack trace here<\/pre>/);
  });

  it("omits the expand button and detail pre when detail is empty", () => {
    const html = noticeHtml(notice());
    expect(html).not.toContain("notice-expand");
    expect(html).not.toContain("notice-detail");
  });

  it("renders a xN multiplier only when count > 1", () => {
    expect(noticeHtml(notice({ count: 3 }))).toContain('<span class="notice-summary">merge failed ×3</span>');
    expect(noticeHtml(notice())).toContain('<span class="notice-summary">merge failed</span>');
  });

  it("escapes summary and detail", () => {
    const html = noticeHtml(notice({ summary: "<b>bad</b>", detail: "<script>alert(1)</script>" }));
    expect(html).not.toContain("<b>bad</b>");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;b&gt;bad&lt;/b&gt;");
    expect(html).toContain("&lt;script&gt;");
  });
});
