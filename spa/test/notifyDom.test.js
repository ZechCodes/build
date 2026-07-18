// @vitest-environment jsdom
// DOM layer of the G2 notification stack: errors persist until dismissed and
// expand to the full text; successes auto-dismiss; repeats dedupe with a count.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { notifyError, notifySuccess, dismissAllNotices } from "../src/core/notify.js";

describe("notify DOM layer", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
    dismissAllNotices();
  });
  afterEach(() => {
    dismissAllNotices();
    vi.useRealTimers();
  });

  it("an error notice persists (no auto-dismiss), expands to full detail, and dismisses on ×", () => {
    const detail = "merge failed because " + "y".repeat(300);
    notifyError("Merge failed", detail);
    vi.advanceTimersByTime(60000); // errors never time out
    const notice = document.querySelector("#notices .notice.error");
    expect(notice).toBeTruthy();
    expect(notice.getAttribute("role")).toBe("alert");

    const detailEl = notice.querySelector(".notice-detail");
    expect(detailEl.hidden).toBe(true);
    notice.querySelector(".notice-expand").click();
    const expanded = document.querySelector("#notices .notice-detail");
    expect(expanded.hidden).toBe(false);
    expect(expanded.textContent).toBe(detail); // full text, never truncated

    document.querySelector("#notices .notice-x").click();
    expect(document.querySelector("#notices .notice")).toBeNull();
  });

  it("an identical repeated error dedupes into one notice with a ×N count", () => {
    notifyError("RPC failed", "boom");
    notifyError("RPC failed", "boom");
    notifyError("RPC failed", "boom");
    const notices = document.querySelectorAll("#notices .notice.error");
    expect(notices).toHaveLength(1);
    expect(notices[0].querySelector(".notice-summary").textContent).toContain("×3");
  });

  it("a success notice auto-dismisses after a few seconds", () => {
    notifySuccess("Merged.");
    expect(document.querySelector("#notices .notice.success")).toBeTruthy();
    vi.advanceTimersByTime(4100);
    expect(document.querySelector("#notices .notice")).toBeNull();
  });

  it("dismissing an error leaves other notices standing", () => {
    notifyError("First failed", "a");
    notifyError("Second failed", "b");
    const first = document.querySelector("#notices .notice");
    first.querySelector(".notice-x").click();
    const remaining = document.querySelectorAll("#notices .notice");
    expect(remaining).toHaveLength(1);
    expect(remaining[0].textContent).toContain("Second failed");
  });
});
