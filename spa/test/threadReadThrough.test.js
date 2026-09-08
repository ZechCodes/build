// @vitest-environment jsdom
// How much of a conversation the reader has actually read.
//
// Reading is per message: a message counts as read once its BOTTOM edge has
// come into view, because that is the moment the reader could have finished it.
// A panel that only reported at the very end of the scroller would leave the
// badge lit over a conversation the reader had read most of, and clear it whole
// over one they had only glanced at.

import { describe, expect, it, afterEach } from "vitest";
import { readThroughSequence } from "../src/core/thread.js";

const VIEWPORT = 300;
const original = Element.prototype.getBoundingClientRect;

/** A timeline of keyed rows whose heights the test names, measured against a
 *  300px viewport from the live scrollTop. */
function timeline(rows) {
  const scroller = document.createElement("div");
  scroller.innerHTML = `<div class="thread-items">${rows.map((row) => row.html).join("")}</div>`;
  scroller.scrollTop = 0;
  document.body.appendChild(scroller);
  Element.prototype.getBoundingClientRect = function () {
    if (this === scroller) return { top: 0, bottom: VIEWPORT, height: VIEWPORT };
    const at = rows.findIndex((row) => row.html === this.outerHTML || this.matches(`[data-probe="${row.probe}"]`));
    if (at < 0) return { top: 0, bottom: 0, height: 0 };
    const above = rows.slice(0, at).reduce((sum, row) => sum + row.height, 0);
    return { top: above - scroller.scrollTop, bottom: above + rows[at].height - scroller.scrollTop };
  };
  return scroller;
}

const messageRow = (sequence, height) => ({
  probe: String(sequence),
  height,
  html: `<div class="thread-message" data-probe="${sequence}" data-sequence="${sequence}"></div>`,
});

const runRow = (from, through, height) => ({
  probe: `run-${from}`,
  height,
  html: `<details class="thread-activity-group" data-probe="run-${from}" data-activity-run="${from}" data-activity-from="${from}" data-activity-through="${through}"></details>`,
});

const lineRow = (height) => ({
  probe: "line",
  height,
  html: '<div class="thread-unread-line" data-probe="line"></div>',
});

describe("readThroughSequence", () => {
  afterEach(() => {
    Element.prototype.getBoundingClientRect = original;
    document.body.innerHTML = "";
  });

  it("reaches the newest message whose bottom is on screen", () => {
    const scroller = timeline([messageRow(4, 100), messageRow(7, 100), messageRow(9, 400)]);
    expect(readThroughSequence(scroller)).toBe(7);
  });

  it("counts a message the reader has scrolled past", () => {
    const scroller = timeline([messageRow(4, 200), messageRow(7, 200), messageRow(9, 400)]);
    scroller.scrollTop = 400; // 4 and 7 are above the viewport now
    expect(readThroughSequence(scroller)).toBe(7);
  });

  it("reaches everything when the reader is at the end", () => {
    const scroller = timeline([messageRow(4, 100), messageRow(7, 100), messageRow(9, 100)]);
    expect(readThroughSequence(scroller)).toBe(9);
  });

  it("reaches nothing when the first message is taller than the viewport", () => {
    const scroller = timeline([messageRow(4, 600), messageRow(7, 100)]);
    expect(readThroughSequence(scroller)).toBe(0);
  });

  it("reads a folded run through the whole span it stands for", () => {
    // A run's box is one row, and reaching the bottom of it means the reader
    // passed every call folded inside — including the half the page cut away.
    const scroller = timeline([messageRow(4, 100), runRow(5, 40, 100), messageRow(41, 400)]);
    expect(readThroughSequence(scroller)).toBe(40);
  });

  it("ignores the rows that stand for nothing anybody said", () => {
    const scroller = timeline([messageRow(4, 100), lineRow(20), messageRow(7, 400)]);
    expect(readThroughSequence(scroller)).toBe(4);
  });

  it("reaches nothing at all in a conversation with nothing in it", () => {
    expect(readThroughSequence(timeline([]))).toBe(0);
    expect(readThroughSequence(null)).toBe(0);
  });
});
