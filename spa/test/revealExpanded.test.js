// @vitest-environment jsdom
// Opening a row, and seeing what opened.
//
// A shut row is one line; an open one can be twenty. The scroll follows it as
// far as it will go — but never so far that the row's own head leaves the top
// of the screen, because a reader who cannot see the line they pressed has lost
// the thread of what they were doing.

import { describe, expect, it, afterEach } from "vitest";
import { revealExpandedRow } from "../src/core/revealExpanded.js";

const original = Element.prototype.getBoundingClientRect;

/** A scroller of a stated height over blocks whose tops and heights the test
 *  names, all measured from the live scrollTop. */
function scrollerOf(height, blocks) {
  const scroller = document.createElement("div");
  scroller.style.overflowY = "auto";
  scroller.scrollTop = 0;
  Object.defineProperty(scroller, "clientHeight", { get: () => height, configurable: true });
  Object.defineProperty(scroller, "scrollHeight", {
    get: () => blocks.reduce((deepest, block) => Math.max(deepest, block.top + block.height), 0),
    configurable: true,
  });
  return scroller;
}

/** jsdom computes no styles, so the test says which elements scroll. */
function measure(boxes) {
  Element.prototype.getBoundingClientRect = function () {
    const box = boxes.get(this);
    if (!box) return { top: 0, bottom: 0, height: 0 };
    return box();
  };
}

describe("revealExpandedRow", () => {
  afterEach(() => {
    Element.prototype.getBoundingClientRect = original;
  });

  /** One scroller 300 tall, one row inside it at `top` and `height` tall. */
  function oneScroller({ top, height, scrollTop = 0 }) {
    const blocks = [{ top, height }];
    const scroller = scrollerOf(300, blocks);
    const row = document.createElement("details");
    scroller.appendChild(row);
    scroller.scrollTop = scrollTop;
    measure(
      new Map([
        [scroller, () => ({ top: 0, bottom: 300, height: 300 })],
        [row, () => ({ top: top - scroller.scrollTop, bottom: top + height - scroller.scrollTop, height })],
      ]),
    );
    return { scroller, row };
  }

  it("brings the whole of an opened row into view", () => {
    const { scroller, row } = oneScroller({ top: 100, height: 150 });
    scroller.scrollTop = 0; // the row runs 100..250 in a 300 viewport: already whole
    revealExpandedRow(row);
    expect(scroller.scrollTop).toBe(0);
  });

  it("scrolls down to the end of a row that hangs below the fold", () => {
    const { scroller, row } = oneScroller({ top: 100, height: 250 });
    revealExpandedRow(row);
    expect(scroller.scrollTop).toBe(50); // 350 - 300, and the head still has 50 of room
  });

  it("never scrolls the row's own head off the top", () => {
    const { scroller, row } = oneScroller({ top: 40, height: 900 });
    revealExpandedRow(row);
    expect(scroller.scrollTop).toBe(40); // the head lands on the top edge, and stops
  });

  it("leaves a row the reader has already scrolled past alone", () => {
    const { scroller, row } = oneScroller({ top: 100, height: 150, scrollTop: 400 });
    revealExpandedRow(row);
    expect(scroller.scrollTop).toBe(400);
  });

  it("does nothing for a row that is in no scroller at all", () => {
    const row = document.createElement("details");
    document.body.appendChild(row);
    measure(new Map([[row, () => ({ top: 0, bottom: 10, height: 10 })]]));
    expect(() => revealExpandedRow(row)).not.toThrow();
  });

  // ---- nested scrollers ------------------------------------------------------
  //
  // An activity run's rows scroll in their own little window, and that window
  // scrolls in the conversation. Opening a row inside one has to move both, or
  // the reader sees the right rows in a box that is itself off the screen.

  it("walks out through every scroller it is nested in", () => {
    const outer = scrollerOf(300, [{ top: 0, height: 900 }]);
    const box = document.createElement("details");
    const inner = scrollerOf(100, [{ top: 0, height: 400 }]);
    const row = document.createElement("details");
    document.body.appendChild(outer);
    outer.appendChild(box);
    box.appendChild(inner);
    inner.appendChild(row);
    // Every rect is where the browser would put it: on the page. The box sits
    // at 250 in a 300-tall conversation and is 120 tall, so its foot hangs 70
    // below the fold. The window inside it is the last 100 of that, and the
    // opened row starts 60 into the window's content and runs 140.
    const innerTop = () => 270 - outer.scrollTop;
    measure(
      new Map([
        [outer, () => ({ top: 0, bottom: 300, height: 300 })],
        [box, () => ({ top: 250 - outer.scrollTop, bottom: 370 - outer.scrollTop, height: 120 })],
        [inner, () => ({ top: innerTop(), bottom: innerTop() + 100, height: 100 })],
        [row, () => ({ top: innerTop() - inner.scrollTop + 60, bottom: innerTop() - inner.scrollTop + 200, height: 140 })],
      ]),
    );
    revealExpandedRow(row);
    expect(inner.scrollTop).toBe(60); // the row's head lands on the box's top edge
    expect(outer.scrollTop).toBe(70); // and the box's foot comes into the conversation
  });
});
