// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { editedTimeLabel, editedTimestamp, refreshEditedTimes } from "../src/core/editedTime.js";

describe("editedTimeLabel", () => {
  const now = Date.UTC(2026, 8, 8, 12);

  it.each([
    [30_000, "Just Now"],
    [60_000, "1 minute ago"],
    [2 * 60_000, "2 minutes ago"],
    [60 * 60_000, "1 hour ago"],
    [2 * 60 * 60_000, "2 hours ago"],
    [24 * 60 * 60_000, "1 day ago"],
    [7 * 24 * 60 * 60_000, "7 days ago"],
    [7 * 24 * 60 * 60_000 + 1, "2026-09-01"],
    [8 * 24 * 60 * 60_000, "2026-08-31"],
  ])("formats %i ms ago", (elapsed, label) => {
    expect(editedTimeLabel(now - elapsed, now)).toBe(label);
  });

  it.each([null, undefined, "", "not-a-date", Infinity, 9e15])("rejects an invalid timestamp: %s", (value) => {
    expect(editedTimestamp(value)).toBe(null);
    expect(editedTimeLabel(value, now)).toBe("");
  });

  it("updates timestamp text without replacing its file or selection", () => {
    const root = document.createElement("div");
    root.innerHTML = `<div class="file"><input checked><time data-edited-at="${now - 30_000}"></time></div>`;
    const file = root.querySelector(".file");
    const input = root.querySelector("input");
    refreshEditedTimes(root, now + 60_000);
    expect(root.querySelector(".file")).toBe(file);
    expect(root.querySelector("input")).toBe(input);
    expect(input.checked).toBe(true);
    expect(root.querySelector("time").textContent).toBe("1 minute ago");
  });

  it("does not mutate a timestamp while its displayed age is unchanged", () => {
    const root = document.createElement("div");
    root.innerHTML = `<time data-edited-at="${now - 10_000}">Just Now</time>`;
    const changes = [];
    const observer = new MutationObserver((records) => changes.push(...records));
    observer.observe(root, { childList: true, subtree: true, characterData: true });
    refreshEditedTimes(root, now + 10_000);
    changes.push(...observer.takeRecords());
    observer.disconnect();
    expect(changes).toEqual([]);
  });
});
