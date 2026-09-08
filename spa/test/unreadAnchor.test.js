// Where the reader's unread begins.
//
// The daemon holds a cursor per conversation and says how much is waiting past
// it; this is the one place those two facts become a place in the timeline —
// the line the divider is ruled above, and the message a repaint opens on.
//
// The line has to hold still. A message is marked read the moment its bottom
// edge comes into view, so a line recomputed from the live cursor on every tick
// would rule itself, clear itself and vanish inside a second. It stays where it
// was put until the reader reaches the end with nothing waiting.

import { describe, expect, it } from "vitest";
import { unreadAnchorSequence } from "../src/core/unreadAnchor.js";

const items = (...sequences) => sequences.map((sequence) => ({ type: "message", data: { sequence } }));

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

  it("takes the line away once the reader reaches the end with nothing waiting", () => {
    const anchor = unreadAnchorSequence({
      held: 7,
      cursor: 9,
      unreadCount: 0,
      items: items(2, 4, 7, 9),
      caughtUp: true,
    });
    expect(anchor).toBeNull();
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

  it("rules nothing when everything the window holds is behind the cursor", () => {
    // The count is honest about history under the tail the window never got.
    expect(unreadAnchorSequence({ cursor: 9, unreadCount: 1, items: items(2, 4, 7, 9) })).toBeNull();
  });
});
