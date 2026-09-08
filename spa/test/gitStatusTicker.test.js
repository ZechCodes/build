// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { recordAnimations, stopRecordingAnimations } from "./motionRecorder.js";
import { gitStatusCells } from "../src/core/gitStatusCells.js";
import { createGitStatusTicker } from "../src/core/gitStatusTicker.js";
import { MOTION_BEAT_MS } from "../src/core/motion.js";

const CELL_WIDTH = 10;

let host;
let ticker;
let started;
let originalBox;

const cellsShown = () => [...host.children].map((cell) => cell.getAttribute("data-cell"));
const cellKeyOf = (run) => run.element.closest("[data-cell]").getAttribute("data-cell");
const keysMoved = () => started.map(cellKeyOf);
const cellNamed = (key) => host.querySelector(`[data-cell="${key}"]`);

/// The recorder's animations only finish when a test says so, and the ticker
/// runs its phases one after another — so finish what is running, let the next
/// phase start, and go round again until the paint is over.
async function drain(work) {
  let over = false;
  const settled = work.then((value) => {
    over = true;
    return value;
  });
  for (let round = 0; round < 16 && !over; round += 1) {
    started.forEach((run) => run.finish());
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  return settled;
}

const show = (stat) => drain(ticker.show(gitStatusCells(stat)));

beforeEach(() => {
  document.body.innerHTML = `<span id="git" hidden></span>`;
  host = document.getElementById("git");
  ticker = createGitStatusTicker(host);
  started = recordAnimations();
  originalBox = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function boxOf() {
    const siblings = this.parentElement ? [...this.parentElement.children] : [];
    const left = Math.max(0, siblings.indexOf(this)) * CELL_WIDTH;
    return { left, top: 0, right: left + CELL_WIDTH, bottom: 12, width: CELL_WIDTH, height: 12 };
  };
});

afterEach(() => {
  stopRecordingAnimations();
  Element.prototype.getBoundingClientRect = originalBox;
  document.body.innerHTML = "";
});

describe("painting a status onto an empty line", () => {
  it("cascades every character in, rightmost first, one beat apart", async () => {
    await show({ ahead: 2 });

    expect(cellsShown()).toEqual(["ahead:glyph", "ahead:d0"]);
    expect(host.textContent).toBe("↑2");
    expect(keysMoved()).toEqual(["ahead:d0", "ahead:glyph"]);
    expect(started[0].options.delay).toBe(0);
    expect(started[1].options.delay).toBe(MOTION_BEAT_MS);
  });

  it("brings each character in from below and from the right, so it travels up and left", async () => {
    await show({ behind: 3 });

    const [from, to] = started[0].keyframes;
    expect(from.opacity).toBe(0);
    expect(from.transform).toBe("translate(0.6em, 100%)");
    expect(to).toEqual({ transform: "translate(0, 0)", opacity: 1 });
  });

  it("shows the line, and leaves no animation on the characters once they are in", async () => {
    await show({ ahead: 1 });

    expect(host.hidden).toBe(false);
    expect(cellNamed("ahead:d0").firstElementChild.getAttribute("style")).toBeFalsy();
  });
});

describe("a number that changed", () => {
  it("rolls the digit up and out while the new one rolls up into its place", async () => {
    await show({ ahead: 1 });
    started.length = 0;

    await show({ ahead: 7 });

    expect(started).toHaveLength(2);
    expect(cellKeyOf(started[1])).toBe("ahead:d0");
    expect(started.map((run) => run.element.textContent)).toEqual(["1", "7"]);
    expect(started[0].keyframes).toEqual([
      { transform: "translateY(0)", opacity: 1 },
      { transform: "translateY(-100%)", opacity: 0 },
    ]);
    expect(started[1].keyframes).toEqual([
      { transform: "translateY(100%)", opacity: 0 },
      { transform: "translateY(0)", opacity: 1 },
    ]);
  });

  it("draws the old value and the new one, and nothing in between", async () => {
    await show({ ahead: 1 });
    const drawn = new Set(["1"]);

    const rolling = ticker.show(gitStatusCells({ ahead: 7 }));
    for (let round = 0; round < 16; round += 1) {
      [...host.querySelectorAll('[data-cell="ahead:d0"] > *')].forEach((glyph) => drawn.add(glyph.textContent));
      started.forEach((run) => run.finish());
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
    await rolling;

    expect([...drawn].sort()).toEqual(["1", "7"]);
    expect(host.textContent).toBe("↑7");
  });

  it("leaves the other characters standing", async () => {
    await show({ ahead: 1, insertions: 4 });
    const glyph = cellNamed("insertions:glyph");
    started.length = 0;

    await show({ ahead: 2, insertions: 4 });

    expect(cellNamed("insertions:glyph")).toBe(glyph);
    expect(started).toHaveLength(2);
    expect(cellKeyOf(started[1])).toBe("ahead:d0");
  });
});

describe("a section arriving beside one already there", () => {
  it("makes room first, sliding what is left of it, and only then cascades the new characters in", async () => {
    await show({ insertions: 4 });
    started.length = 0;

    await show({ insertions: 4, deletions: 1 });

    expect(cellsShown()).toEqual(["insertions:glyph", "insertions:d0", "deletions:glyph", "deletions:d0"]);
    expect(host.textContent).toBe("+4−1");
    expect(keysMoved()).toEqual(["deletions:d0", "deletions:glyph"]);
  });

  it("slides the characters whose place moved, from where they were to where they are", async () => {
    await show({ insertions: 4 });
    started.length = 0;

    await show({ ahead: 2, insertions: 4 });

    const slides = started.filter((run) => run.element.hasAttribute("data-cell"));
    expect(slides.map(cellKeyOf)).toEqual(["insertions:glyph", "insertions:d0"]);
    expect(slides[0].keyframes).toEqual([{ transform: "translateX(-20px)" }, { transform: "translateX(0)" }]);
    expect(started.indexOf(slides[0])).toBeLessThan(started.findIndex((run) => cellKeyOf(run) === "ahead:d0"));
  });
});

describe("a section that is gone", () => {
  it("cascades its characters out leftmost first, up and to the right", async () => {
    await show({ ahead: 12, insertions: 4 });
    started.length = 0;

    await show({ insertions: 4 });

    expect(cellsShown()).toEqual(["insertions:glyph", "insertions:d0"]);
    expect(host.textContent).toBe("+4");
    const exits = started.slice(0, 3);
    expect(exits.map(cellKeyOf)).toEqual(["ahead:glyph", "ahead:d1", "ahead:d0"]);
    expect(exits[0].keyframes[1]).toEqual({ transform: "translate(0.6em, -100%)", opacity: 0 });
    expect(exits.map((run) => run.options.delay)).toEqual([0, MOTION_BEAT_MS, 2 * MOTION_BEAT_MS]);
  });

  it("slides what is left only after the old characters have gone", async () => {
    await show({ ahead: 2, insertions: 4 });
    started.length = 0;

    await show({ insertions: 4 });

    const slides = started.filter((run) => run.element.hasAttribute("data-cell"));
    expect(slides.map(cellKeyOf)).toEqual(["insertions:glyph", "insertions:d0"]);
    expect(started.indexOf(slides[0])).toBeGreaterThan(1);
  });

  it("hides the line once the last character has left", async () => {
    await show({ ahead: 2 });

    await show(null);

    expect(cellsShown()).toEqual([]);
    expect(host.hidden).toBe(true);
  });
});

describe("a status that changes while the line is still moving", () => {
  it("holds the update back and paints the newest one when the line settles", async () => {
    const first = ticker.show(gitStatusCells({ ahead: 1 }));
    ticker.show(gitStatusCells({ ahead: 2 }));
    ticker.show(gitStatusCells({ ahead: 3 }));

    await drain(first);
    await drain(ticker.settled());

    expect(host.textContent).toBe("↑3");
  });

  it("draws only the newest of the updates it held, never the ones it overtook", async () => {
    await show({ ahead: 1 });
    started.length = 0;

    const rolling = ticker.show(gitStatusCells({ ahead: 2 }));
    ticker.show(gitStatusCells({ ahead: 3 }));
    ticker.show(gitStatusCells({ ahead: 4 }));
    await drain(rolling);
    await drain(ticker.settled());

    expect(host.textContent).toBe("↑4");
    expect(started.map((run) => run.element.textContent)).not.toContain("3");
  });

  it("counts as populated while it is still moving, so the row it sits in stays open", async () => {
    await show({ ahead: 2 });

    const leaving = ticker.show(gitStatusCells(null));
    expect(ticker.populated()).toBe(true);

    await drain(leaving);
    expect(ticker.populated()).toBe(false);
  });
});

describe("when the reader asked for less motion", () => {
  it("snaps to the new status without animating anything", async () => {
    stopRecordingAnimations();

    await ticker.show(gitStatusCells({ ahead: 2, insertions: 4 }));
    expect(host.textContent).toBe("↑2+4");
    expect(host.hidden).toBe(false);

    await ticker.show(gitStatusCells({ insertions: 5 }));
    expect(host.textContent).toBe("+5");
    expect(cellsShown()).toEqual(["insertions:glyph", "insertions:d0"]);

    await ticker.show(gitStatusCells(null));
    expect(host.hidden).toBe(true);
  });
});
