// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { editedTimeLabel, editedTimestamp, refreshEditedTimes, watchEditedTimes } from "../src/core/editedTime.js";

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

describe("the edited-time refresh lifecycle", () => {
  const now = Date.UTC(2026, 8, 8, 12);
  let root;
  let watcher;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(now);
    root = document.createElement("div");
    root.innerHTML = `<time data-edited-at="${now}"></time>`;
  });

  afterEach(() => {
    watcher?.dispose();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("pauses a hidden document and refreshes before restarting its timer", () => {
    watcher = watchEditedTimes(root);
    const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(true);
    document.dispatchEvent(new Event("visibilitychange"));

    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(2 * 60_000);
    expect(root.textContent).toBe("Just Now");
    hidden.mockReturnValue(false);
    document.dispatchEvent(new Event("visibilitychange"));

    expect(root.textContent).toBe("2 minutes ago");
    expect(vi.getTimerCount()).toBe(1);
  });

  it("refreshes immediately on pageshow without duplicating its timer", () => {
    watcher = watchEditedTimes(root);
    vi.setSystemTime(now + 3 * 60_000);

    window.dispatchEvent(new Event("pageshow"));
    window.dispatchEvent(new Event("pageshow"));

    expect(root.textContent).toBe("3 minutes ago");
    expect(vi.getTimerCount()).toBe(1);
  });

  it("keeps a hidden pane paused across foreground events, then refreshes on reveal", () => {
    watcher = watchEditedTimes(root);
    watcher.setVisible(false);
    vi.setSystemTime(now + 2 * 60_000);
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("pageshow"));

    expect(root.textContent).toBe("Just Now");
    expect(vi.getTimerCount()).toBe(0);
    watcher.setVisible(true);
    expect(root.textContent).toBe("2 minutes ago");
    expect(vi.getTimerCount()).toBe(1);
  });

  it("cannot restart after disposal", () => {
    watcher = watchEditedTimes(root);
    watcher.setVisible(false);
    watcher.dispose();
    vi.setSystemTime(now + 2 * 60_000);

    watcher.setVisible(true);
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("pageshow"));

    expect(root.textContent).toBe("Just Now");
    expect(vi.getTimerCount()).toBe(0);
  });
});
