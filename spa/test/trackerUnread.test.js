import { afterEach, describe, expect, it, vi } from "vitest";
import { createUnreadMarker, UNREAD_LINE_GRACE_MS } from "../src/core/unreadAnchor.js";
import { issueUnreadKey, issueUnreadReading, issueUnreadRules } from "../src/core/trackerUnread.js";

const id = (kind, suffix) => `${kind}-01M37FGQD48628P29BG1A4BB${suffix}`;
// An event row is a move: news, which counts (#183), where filing would not.
const row = (kind, suffix, actor = "agent") => ({ key: id(kind, suffix), actor: { kind: actor },
  ...(kind === "ic" ? { type: "comment" } : { type: "event", kind: "moved" }) });

describe("issue unread marker", () => {
  afterEach(() => vi.useRealTimers());

  it("starts above the oldest other person's event and skips the reader's own comments", () => {
    const rows = [row("ie", "01", "user"), row("ic", "02", "user"), row("ie", "03"), row("ic", "04")];
    const marker = createUnreadMarker(() => {}, issueUnreadRules);
    expect(marker.update(issueUnreadReading(rows, id("ie", "01")))).toBe(issueUnreadKey(id("ie", "03")));
    marker.leave();
  });

  it("never reads a timeline-invented key as a mark or an anchor", () => {
    const rows = [{ key: "comment-3", actor: { kind: "agent" } }, row("ic", "04")];
    const reading = issueUnreadReading(rows, "event-0", "comment-3");
    expect(reading.cursor).toBe("");
    expect(reading.readThrough).toBe("");
    expect(reading.unreadCount).toBe(1);
    const marker = createUnreadMarker(() => {}, issueUnreadRules);
    expect(marker.update(reading)).toBe(issueUnreadKey(id("ic", "04")));
    marker.leave();
  });

  it("holds for 60 seconds after the visit reads through and rejects a stale repaint", () => {
    vi.useFakeTimers();
    const expired = vi.fn();
    const marker = createUnreadMarker(expired, issueUnreadRules);
    const rows = [row("ie", "01", "user"), row("ic", "02"), row("ic", "03")];
    const before = issueUnreadReading(rows, id("ie", "01"));
    expect(marker.update(before)).toBe(issueUnreadKey(id("ic", "02")));
    expect(marker.update(issueUnreadReading(rows, id("ie", "01"), id("ic", "03")))).toBe(issueUnreadKey(id("ic", "02")));
    vi.advanceTimersByTime(UNREAD_LINE_GRACE_MS - 1);
    expect(expired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(expired).toHaveBeenCalledOnce();
    expect(marker.update(before)).toBeNull();
    expect(marker.update(issueUnreadReading([...rows, row("ic", "04")], id("ic", "03"))))
      .toBe(issueUnreadKey(id("ic", "04")));
    marker.leave();
  });
});
