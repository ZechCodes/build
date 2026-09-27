import { afterEach, describe, expect, it, vi } from "vitest";
import { createUnreadMarker, UNREAD_LINE_GRACE_MS } from "../src/core/unreadAnchor.js";
import { taskUnreadKey, taskUnreadReading, taskUnreadRules } from "../src/core/trackerUnread.js";

const id = (kind, suffix) => `${kind}-01M37FGQD48628P29BG1A4BB${suffix}`;
// An event row is a move: news, which counts (#183), where filing would not.
const row = (kind, suffix, actor = "agent") => ({ key: id(kind, suffix), actor: { kind: actor },
  ...(kind === "tc" ? { type: "comment" } : { type: "event", kind: "moved" }) });

describe("task unread marker", () => {
  afterEach(() => vi.useRealTimers());

  it("starts above the oldest other person's event and skips the reader's own comments", () => {
    const rows = [row("te", "01", "user"), row("tc", "02", "user"), row("te", "03"), row("tc", "04")];
    const marker = createUnreadMarker(() => {}, taskUnreadRules);
    expect(marker.update(taskUnreadReading(rows, id("te", "01")))).toBe(taskUnreadKey(id("te", "03")));
    marker.leave();
  });

  it("never reads a timeline-invented key as a mark or an anchor", () => {
    const rows = [{ key: "comment-3", actor: { kind: "agent" } }, row("tc", "04")];
    const reading = taskUnreadReading(rows, "event-0", "comment-3");
    expect(reading.cursor).toBe("");
    expect(reading.readThrough).toBe("");
    expect(reading.unreadCount).toBe(1);
    const marker = createUnreadMarker(() => {}, taskUnreadRules);
    expect(marker.update(reading)).toBe(taskUnreadKey(id("tc", "04")));
    marker.leave();
  });

  it("holds for 60 seconds after the visit reads through and rejects a stale repaint", () => {
    vi.useFakeTimers();
    const expired = vi.fn();
    const marker = createUnreadMarker(expired, taskUnreadRules);
    const rows = [row("te", "01", "user"), row("tc", "02"), row("tc", "03")];
    const before = taskUnreadReading(rows, id("te", "01"));
    expect(marker.update(before)).toBe(taskUnreadKey(id("tc", "02")));
    expect(marker.update(taskUnreadReading(rows, id("te", "01"), id("tc", "03")))).toBe(taskUnreadKey(id("tc", "02")));
    vi.advanceTimersByTime(UNREAD_LINE_GRACE_MS - 1);
    expect(expired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(expired).toHaveBeenCalledOnce();
    expect(marker.update(before)).toBeNull();
    expect(marker.update(taskUnreadReading([...rows, row("tc", "04")], id("tc", "03"))))
      .toBe(taskUnreadKey(id("tc", "04")));
    marker.leave();
  });
});

describe("a mark cached before tasks were renamed (#190)", () => {
  it("reads an old comment or event id by the same clock as a new one", () => {
    expect(taskUnreadKey("ic-01M37FGQD48628P29BG1A4BB01")).toBe("01M37FGQD48628P29BG1A4BB01");
    const rows = [row("tc", "02"), row("te", "03")];
    expect(taskUnreadReading(rows, "ie-01M37FGQD48628P29BG1A4BB02").unreadCount).toBe(1);
  });
});
