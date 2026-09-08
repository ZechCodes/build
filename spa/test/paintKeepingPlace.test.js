// @vitest-environment jsdom
// Keeping the reader's place across a paint. A conversation pins the newest
// message to the bottom; a diff holds the file the reader is looking at still
// while the stack grows above and below it. Both are the same lever — the
// scroll position around a repaint — so both live behind one helper, and
// neither touches the scroller for a paint that wrote nothing.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { anchorTop, followConversation, paintKeepingPlace } from "../src/core/paintKeepingPlace.js";

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

  describe("a conversation, following what the reader has not read", () => {
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
      paintKeepingPlace(scroller, paintThread(scroller), { opening, policy: followConversation() });
      expect(scroller.scrollTop).toBe(1000);
    });

    it("touches nothing when a paint that would have carried history wrote none", () => {
      const scroller = tallScroller();
      paintKeepingPlace(scroller, paintThread(scroller), { opening, policy: followConversation() });
      const writes = watchScrollTop(scroller);
      paintKeepingPlace(scroller, () => {}, { opening, policy: followConversation({ olderItemsPrepended: true }) });
      expect(writes).toEqual([]);
    });

    it("leaves a reader who scrolled up where they were", () => {
      const scroller = tallScroller();
      paintKeepingPlace(scroller, paintThread(scroller), { opening, policy: followConversation() });
      scroller.scrollTop = 120;
      paintKeepingPlace(scroller, paintThread(scroller), { opening, policy: followConversation() });
      expect(scroller.scrollTop).toBe(120);
    });

    // ---- landing on what has not been read ---------------------------------
    //
    // The bottom is where a conversation with nothing waiting goes. A
    // conversation with an unread line in it goes to the LINE instead: the
    // reader came for the first thing they have not read, not the last thing
    // said, and reading a burst starts at its beginning.

    /** A 300px conversation over rows whose heights the test names, measured
     *  from the live scrollTop so a policy's write can be read back. The
     *  harness never clamps — a policy that scrolls past the end has to be
     *  visible as one. */
    const conversation = (rows) => {
      const scroller = document.createElement("div");
      const total = () => rows.reduce((sum, row) => sum + row.height, 0);
      Object.defineProperty(scroller, "scrollHeight", { get: total, configurable: true });
      Object.defineProperty(scroller, "clientHeight", { get: () => VIEWPORT, configurable: true });
      scroller.scrollTop = 0;
      document.body.appendChild(scroller);
      Element.prototype.getBoundingClientRect = function () {
        if (this === scroller) return { top: 0, bottom: VIEWPORT, height: VIEWPORT };
        const at = rows.findIndex((row) => row.key === this.getAttribute("data-key"));
        if (at < 0) return { top: 0, bottom: 0, height: 0 };
        const above = rows.slice(0, at).reduce((sum, row) => sum + row.height, 0);
        return { top: above - scroller.scrollTop, bottom: above + rows[at].height - scroller.scrollTop };
      };
      const paint = () => {
        scroller.innerHTML = rows
          .map((row) => `<div class="${row.line ? "thread-unread-line" : "msg"}" data-key="${row.key}"></div>`)
          .join("");
      };
      return { scroller, paint };
    };

    const followUnread = (extra) => followConversation({ unreadSelector: ".thread-unread-line", ...extra });

    it("opens on the unread line rather than the newest message", () => {
      const { scroller, paint } = conversation([
        { key: "a", height: 400 },
        { key: "line", height: 20, line: true },
        { key: "b", height: 400 },
      ]);
      paintKeepingPlace(scroller, paint, { opening, policy: followUnread() });
      expect(scroller.scrollTop).toBe(400);
    });

    it("opens at the bottom when there is nothing unread", () => {
      const { scroller, paint } = conversation([
        { key: "a", height: 400 },
        { key: "b", height: 400 },
      ]);
      paintKeepingPlace(scroller, paint, { opening, policy: followUnread() });
      expect(scroller.scrollTop).toBe(800);
    });

    it("takes a reader who is at the end to what has just arrived", () => {
      const { scroller, paint } = conversation([
        { key: "a", height: 400 },
        { key: "line", height: 20, line: true },
        { key: "b", height: 400 },
      ]);
      scroller.scrollTop = 520; // scrollHeight - clientHeight: at the end
      paintKeepingPlace(scroller, paint, { opening: () => false, policy: followUnread() });
      expect(scroller.scrollTop).toBe(400);
    });

    it("leaves a reader who scrolled up where they are, line or no line", () => {
      const { scroller, paint } = conversation([
        { key: "a", height: 400 },
        { key: "line", height: 20, line: true },
        { key: "b", height: 400 },
      ]);
      scroller.scrollTop = 60;
      paintKeepingPlace(scroller, paint, { opening: () => false, policy: followUnread() });
      expect(scroller.scrollTop).toBe(60);
    });

    it("never scrolls past the end to put a line at the top", () => {
      // The last message is shorter than the viewport, so its line cannot reach
      // the top — and a scroller asked for more than it has just shows the end.
      const { scroller, paint } = conversation([
        { key: "a", height: 700 },
        { key: "line", height: 20, line: true },
        { key: "b", height: 40 },
      ]);
      paintKeepingPlace(scroller, paint, { opening, policy: followUnread() });
      expect(scroller.scrollTop).toBe(460);
    });

    it("holds a reader reading history still when older items land above the line", () => {
      const rows = [
        { key: "a", height: 400 },
        { key: "line", height: 20, line: true },
        { key: "b", height: 400 },
      ];
      const { scroller, paint } = conversation(rows);
      paintKeepingPlace(scroller, paint, { opening, policy: followUnread() });
      scroller.scrollTop = 0; // the reader reads all the way back

      paintKeepingPlace(
        scroller,
        () => {
          rows.unshift({ key: "old", height: 600 });
          paint();
        },
        { opening: () => false, policy: followUnread({ olderItemsPrepended: true }) },
      );
      expect(scroller.scrollTop).toBe(600);
    });
  });
});
