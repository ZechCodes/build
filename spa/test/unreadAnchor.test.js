// Where the reader's unread begins.
//
// The daemon holds a cursor per conversation and says how much is waiting past
// it; this is the one place those two facts become a place in the timeline —
// the line the divider is ruled above, and the message a repaint opens on.
//
// The line has to hold still. A message is marked read the moment its bottom
// edge comes into view, so a line recomputed from the live cursor on every tick
// would rule itself, clear itself and vanish inside a second. It stays where it
// was put until the grace period after reading the latest agent reply ends.

import { afterEach, describe, expect, it, vi } from "vitest";
import { createUnreadMarker, unreadAnchorSequence } from "../src/core/unreadAnchor.js";

const items = (...sequences) => sequences.map((sequence) => ({ type: "message", data: { sequence, role: "agent" } }));

describe("unreadAnchorSequence", () => {
  it("rules the line above the first item past the cursor", () => {
    const anchor = unreadAnchorSequence({ cursor: 4, unreadCount: 2, items: items(2, 4, 7, 9) });
    expect(anchor).toBe(7);
  });

  it("has nothing to rule when nothing is waiting", () => {
    const anchor = unreadAnchorSequence({ cursor: 9, unreadCount: 0, items: items(2, 4, 7, 9) });
    expect(anchor).toBeNull();
  });

  it("rules the line above the whole conversation when the reader has never read it", () => {
    const anchor = unreadAnchorSequence({ cursor: 0, unreadCount: 3, items: items(2, 4, 7) });
    expect(anchor).toBe(2);
  });

  it("holds the line still while the reader reads past it", () => {
    // The cursor has run to the end — the reader's viewport reached every one
    // of them — and the line stays above where they started reading.
    const anchor = unreadAnchorSequence({ held: 7, cursor: 9, unreadCount: 0, items: items(2, 4, 7, 9) });
    expect(anchor).toBe(7);
  });

  it("holds the line still when more arrives below it", () => {
    const anchor = unreadAnchorSequence({ held: 7, cursor: 7, unreadCount: 2, items: items(2, 4, 7, 9, 11) });
    expect(anchor).toBe(7);
  });

  it("keeps the line after the reader reaches the end for the visit timer", () => {
    const anchor = unreadAnchorSequence({
      held: 7,
      cursor: 9,
      unreadCount: 0,
      items: items(2, 4, 7, 9),
      caughtUp: true,
    });
    expect(anchor).toBe(7);
  });

  it("keeps asking while the reader is at the end and something is still waiting", () => {
    // At the bottom of a window is not the end of the conversation: an unread
    // message under the tail is still unread.
    const anchor = unreadAnchorSequence({
      held: 7,
      cursor: 4,
      unreadCount: 1,
      items: items(2, 4, 7, 9),
      caughtUp: true,
    });
    expect(anchor).toBe(7);
  });

  it("rules a fresh line for what arrives after the reader caught up", () => {
    const anchor = unreadAnchorSequence({ held: null, cursor: 9, unreadCount: 1, items: items(2, 4, 7, 9, 11) });
    expect(anchor).toBe(11);
  });

  it("re-rules the line when the one it held has been paged out of the window", () => {
    const anchor = unreadAnchorSequence({ held: 2, cursor: 7, unreadCount: 1, items: items(7, 9) });
    expect(anchor).toBe(9);
  });

  it("keeps the line it holds across a paint with nothing loaded", () => {
    const anchor = unreadAnchorSequence({ held: 7, cursor: 4, unreadCount: 1, items: [] });
    expect(anchor).toBe(7);
  });

  it("rules nothing over a conversation with nothing in it", () => {
    expect(unreadAnchorSequence({ cursor: 0, unreadCount: 1, items: [] })).toBeNull();
  });

  it("rules nothing when the daemon has not said where the cursor is", () => {
    // A bridge that predates the cursor on the wire says nothing about it, and
    // reading that silence as "read nothing" would rule a line above the whole
    // window. No line is the honest answer, and the old landing — the newest
    // message — is what the conversation falls back to.
    const anchor = unreadAnchorSequence({ unreadCount: 2, items: items(2, 4, 7) });
    expect(anchor).toBeNull();
  });

  it("rules nothing when everything the window holds is behind the cursor", () => {
    // The count is honest about history under the tail the window never got.
    expect(unreadAnchorSequence({ cursor: 9, unreadCount: 1, items: items(2, 4, 7, 9) })).toBeNull();
  });
});


describe("unread marker grace period", () => {
  afterEach(() => vi.useRealTimers());
  const reading = (overrides = {}) => ({ cursor: 4, unreadCount: 2, items: items(4, 7, 9), ...overrides });
  const marker = () => {
    vi.useFakeTimers();
    const expired = vi.fn();
    return { visit: createUnreadMarker(expired), expired };
  };

  it("ignores user messages and events before the oldest unread agent reply", () => {
    const mixed = [{ type: "message", data: { sequence: 5, role: "user" } },
      { type: "event", data: { sequence: 6 } }, ...items(7, 9)];
    expect(unreadAnchorSequence(reading({ items: mixed }))).toBe(7);
  });

  it("waits until the latest agent reply is read then keeps the line for exactly 60 seconds", () => {
    const { visit, expired } = marker();
    expect(visit.update(reading({ readThrough: 7 }))).toBe(7);
    vi.advanceTimersByTime(60_000);
    expect(expired).not.toHaveBeenCalled();
    expect(visit.update(reading({ readThrough: 9 }))).toBe(7);
    vi.advanceTimersByTime(59_999);
    expect(visit.update(reading({ cursor: 9, unreadCount: 0, readThrough: 0 }))).toBe(7);
    expect(expired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(expired).toHaveBeenCalledOnce();
    expect(visit.update(reading())).toBeNull();
  });

  it("clears immediately on leaving and cannot revive from a stale read cursor", () => {
    const { visit, expired } = marker();
    visit.update(reading({ readThrough: 9 }));
    vi.advanceTimersByTime(20_000);
    visit.leave();
    expect(visit.update(reading())).toBeNull();
    vi.advanceTimersByTime(60_000);
    expect(expired).not.toHaveBeenCalled();
    expect(visit.update(reading({ unreadCount: 3, items: items(4, 7, 9, 12) }))).toBe(12);
    visit.leave();
  });

  it("restarts the grace period when a new reply is read", () => {
    const { visit, expired } = marker();
    visit.update(reading({ readThrough: 9 }));
    vi.advanceTimersByTime(40_000);
    const newer = reading({ unreadCount: 3, items: items(4, 7, 9, 12) });
    expect(visit.update(newer)).toBe(7);
    vi.advanceTimersByTime(60_000);
    expect(expired).not.toHaveBeenCalled();
    expect(visit.update({ ...newer, readThrough: 12 })).toBe(7);
    vi.advanceTimersByTime(59_999);
    expect(expired).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(expired).toHaveBeenCalledOnce();
  });

  it("does not mistake a read tail window for all unread messages being read", () => {
    const { visit, expired } = marker();
    visit.update(reading({ items: items(9), readThrough: 9 }));
    vi.advanceTimersByTime(60_000);
    expect(expired).not.toHaveBeenCalled();
    expect(visit.update(reading({ readThrough: 9 }))).toBe(9);
    vi.advanceTimersByTime(60_000);
    expect(expired).toHaveBeenCalledOnce();
  });
});
