// @vitest-environment jsdom
// Keeping the reader's place across a paint. A conversation pins the newest
// message to the bottom; a diff holds the file the reader is looking at still
// while the stack grows above and below it. Both are the same lever — the
// scroll position around a repaint — so both live behind one helper, and
// neither touches the scroller for a paint that wrote nothing.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { anchorTop, paintKeepingPlace, pinToBottom } from "../src/core/paintKeepingPlace.js";

const VIEWPORT = 300;

/** jsdom has no layout, so the test states one: a scroller 300px tall over a
 *  stack of keyed blocks whose heights it names. */
function stackLayout() {
  const heights = new Map();
  const scroller = document.createElement("div");
  scroller.className = "scroller";
  document.body.appendChild(scroller);
  const measure = function () {
    if (this === scroller) return { top: 0, bottom: VIEWPORT, height: VIEWPORT };
    const key = this.getAttribute && this.getAttribute("data-key");
    if (!key || !heights.has(key)) return { top: 0, bottom: 0, height: 0 };
    let above = 0;
    for (const [name, height] of heights) {
      if (name === key) break;
      above += height;
    }
    const top = above - scroller.scrollTop;
    return { top, bottom: top + heights.get(key), height: heights.get(key) };
  };
  const original = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = measure;
  return {
    scroller,
    heights,
    restore: () => {
      Element.prototype.getBoundingClientRect = original;
    },
  };
}

const stackHtml = (keys) => keys.map((key) => `<div class="file" data-key="${key}">${key}</div>`).join("");

/** Every write to `scrollTop`, so a tick that wrote nothing can be told from
 *  one that wrote back the number it already held. */
function watchScrollTop(element) {
  const writes = [];
  let held = element.scrollTop;
  Object.defineProperty(element, "scrollTop", {
    configurable: true,
    get: () => held,
    set: (value) => {
      writes.push(value);
      held = value;
    },
  });
  return writes;
}

describe("paintKeepingPlace", () => {
  let layout;

  beforeEach(() => {
    document.body.innerHTML = "";
    layout = stackLayout();
  });

  afterEach(() => {
    layout.restore();
  });

  const openDiff = (scroller) => !scroller.querySelector(".file[data-key]");
  const diffPolicy = anchorTop(".file[data-key]");

  const paintStack = (scroller, keys) => () => {
    scroller.innerHTML = stackHtml(keys);
  };

  it("paints and does nothing else when there is no scroller", () => {
    const host = document.createElement("div");
    paintKeepingPlace(null, paintStack(host, ["a"]), { opening: openDiff, policy: diffPolicy });
    expect(host.querySelector(".file")).toBeTruthy();
  });

  describe("a diff, anchored to the file under the reader", () => {
    const openTwoFiles = () => {
      const { scroller, heights } = layout;
      heights.set("a", 400);
      heights.set("b", 400);
      paintKeepingPlace(scroller, paintStack(scroller, ["a", "b"]), { opening: openDiff, policy: diffPolicy });
      scroller.scrollTop = 400; // the reader scrolled to the head of b
    };

    it("holds the file being read still when the one above it grows", () => {
      const { scroller, heights } = layout;
      openTwoFiles();
      paintKeepingPlace(
        scroller,
        () => {
          heights.set("a", 600);
          paintStack(scroller, ["a", "b"])();
        },
        { opening: openDiff, policy: diffPolicy },
      );
      expect(scroller.scrollTop).toBe(600);
    });

    it("leaves the reader where they are when the file they are reading changes in place", () => {
      const { scroller } = layout;
      openTwoFiles();
      const writes = watchScrollTop(scroller);
      paintKeepingPlace(scroller, paintStack(scroller, ["a", "b"]), { opening: openDiff, policy: diffPolicy });
      expect(scroller.scrollTop).toBe(400);
      expect(writes).toEqual([]);
    });

    it("touches nothing at all when the paint wrote nothing", () => {
      const { scroller } = layout;
      openTwoFiles();
      const writes = watchScrollTop(scroller);
      paintKeepingPlace(scroller, () => {}, { opening: openDiff, policy: diffPolicy });
      expect(writes).toEqual([]);
    });

    it("leaves the scroll alone when the file it was holding is gone", () => {
      const { scroller, heights } = layout;
      openTwoFiles();
      const writes = watchScrollTop(scroller);
      paintKeepingPlace(
        scroller,
        () => {
          heights.delete("b");
          paintStack(scroller, ["a"])();
        },
        { opening: openDiff, policy: diffPolicy },
      );
      expect(writes).toEqual([]);
    });

    it("holds nothing on the paint that opens the stack", () => {
      const { scroller, heights } = layout;
      heights.set("a", 400);
      const writes = watchScrollTop(scroller);
      paintKeepingPlace(scroller, paintStack(scroller, ["a"]), { opening: openDiff, policy: diffPolicy });
      expect(writes).toEqual([]);
    });
  });

  describe("a conversation, pinned to its newest message", () => {
    const opening = (scroller) => !scroller.querySelector(".msg");
    const paintThread = (scroller) => () => {
      scroller.innerHTML = '<div class="msg">landed</div>';
    };

    const tallScroller = () => {
      const scroller = document.createElement("div");
      Object.defineProperty(scroller, "scrollHeight", { get: () => 1000, configurable: true });
      Object.defineProperty(scroller, "clientHeight", { get: () => 300, configurable: true });
      scroller.scrollTop = 0;
      document.body.appendChild(scroller);
      return scroller;
    };

    it("opens at the bottom", () => {
      const scroller = tallScroller();
      paintKeepingPlace(scroller, paintThread(scroller), { opening, policy: pinToBottom() });
      expect(scroller.scrollTop).toBe(1000);
    });

    it("leaves a reader who scrolled up where they were", () => {
      const scroller = tallScroller();
      paintKeepingPlace(scroller, paintThread(scroller), { opening, policy: pinToBottom() });
      scroller.scrollTop = 120;
      paintKeepingPlace(scroller, paintThread(scroller), { opening, policy: pinToBottom() });
      expect(scroller.scrollTop).toBe(120);
    });
  });
});
