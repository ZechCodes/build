// @vitest-environment jsdom
// Keeping the reader's place across a paint. A conversation pins the newest
// message to the bottom; a diff holds the file the reader is looking at still
// while the stack grows above and below it. Both are the same lever — the
// scroll position around a repaint — so both live behind one helper, and
// neither touches the scroller for a paint that wrote nothing.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { anchorTop, followConversation, paintKeepingPlace, wireReaderMotion, readerIsMoving } from "../src/core/paintKeepingPlace.js";

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
        const at = rows.findIndex((row) => row.key === this.closest("[data-key]")?.getAttribute("data-key"));
        if (at < 0) return { top: 0, bottom: 0, height: 0 };
        const above = rows.slice(0, at).reduce((sum, row) => sum + row.height, 0);
        const inset = this.tagName === "P" ? rows[at].inset || 0 : 0;
        return { top: above + inset - scroller.scrollTop, bottom: above + rows[at].height - scroller.scrollTop };
      };
      const paint = () => {
        scroller.innerHTML = rows
          .map((row) => `<div class="${row.line ? "thread-unread-line" : "msg"}" data-key="${row.key}">${row.inset == null ? "" : "<p>Reading</p>"}</div>`)
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
      expect(scroller.scrollTop).toBe(388); // the line, less a sliver of what came before
    });

    it("opens at the bottom when there is nothing unread", () => {
      const { scroller, paint } = conversation([
        { key: "a", height: 400 },
        { key: "b", height: 400 },
      ]);
      paintKeepingPlace(scroller, paint, { opening, policy: followUnread() });
      expect(scroller.scrollTop).toBe(800);
    });

    // Following means the end: what arrived is drawn under what they were
    // reading, and the line is where the panel OPENS, not where each paint
    // drags them back to.
    it("keeps a reader who is at the end at the end as things arrive", () => {
      const { scroller, paint } = conversation([
        { key: "a", height: 400 },
        { key: "line", height: 20, line: true },
        { key: "b", height: 400 },
      ]);
      scroller.scrollTop = 520; // scrollHeight - clientHeight: at the end
      paintKeepingPlace(scroller, paint, { opening: () => false, policy: followUnread() });
      expect(scroller.scrollTop).toBe(820);
    });

    it("writes nothing when the place it would keep is the place the reader is", () => {
      const { scroller, paint } = conversation([
        { key: "a", height: 400 },
        { key: "b", height: 400 },
      ]);
      paintKeepingPlace(scroller, paint, { opening, policy: followUnread() });
      scroller.scrollTop = 60;
      const writes = watchScrollTop(scroller);
      paintKeepingPlace(scroller, paint, { opening: () => false, policy: followUnread() });
      expect(writes).toEqual([]);
    });

    it("anchors halfway through a long row when content grows above it, but not below it", () => {
      const rows = [{ key: "before", height: 200 }, { key: "reading", height: 1000 }, { key: "after", height: 200 }];
      const { scroller, paint } = conversation(rows);
      paint();
      scroller.scrollTop = 600;
      const readingTop = () => scroller.querySelector('[data-key="reading"]').getBoundingClientRect().top;
      const before = readingTop();
      paintKeepingPlace(scroller, () => { rows[0].height += 100; paint(); }, { opening: () => false, policy: followConversation() });
      expect(readingTop()).toBe(before);
      expect(scroller.scrollTop).toBe(700);
      paintKeepingPlace(scroller, () => { rows[2].height += 100; paint(); }, { opening: () => false, policy: followConversation() });
      expect(readingTop()).toBe(before);
      expect(scroller.scrollTop).toBe(700);
      // The same total height can still move the content being read.
      paintKeepingPlace(scroller, () => { rows[0].height += 100; rows[2].height -= 100; paint(); }, { opening: () => false, policy: followConversation() });
      expect(readingTop()).toBe(before);
      expect(scroller.scrollTop).toBe(800);
    });

    it("keeps the visible paragraph still when metadata grows inside its row", () => {
      const rows = [{ key: "before", height: 200 }, { key: "reading", height: 1000, inset: 100 }, { key: "after", height: 200 }];
      const { scroller, paint } = conversation(rows);
      paint();
      scroller.scrollTop = 600;
      const contentTop = () => scroller.querySelector("p").getBoundingClientRect().top;
      const before = contentTop();
      paintKeepingPlace(scroller, () => { rows[1].inset += 100; rows[1].height += 100; paint(); }, { opening: () => false, policy: followConversation() });
      expect(contentTop()).toBe(before);
      expect(scroller.scrollTop).toBe(700);
    });

    for (const [kind, content] of [
      ["table cells", '<div class="mdtable"><table><tbody><tr><td data-reading-leaf>Reading table cell</td></tr></tbody></table></div>'],
      ["nested quote content", '<blockquote><ul><li><blockquote><h2 data-reading-leaf>Reading heading</h2></blockquote></li></ul></blockquote>'],
      ["rules", '<hr data-reading-leaf>'],
      ["attachment sibling", '</div><div class="thread-attachments"><figure class="thread-attachment-figure"><img data-reading-leaf><figcaption>Saved image</figcaption></figure></div><div>'],
      ["body child fallback", '<div><span data-reading-leaf>Visible body child</span></div>'],
    ]) for (const nativeAnchoring of [false, true]) {
      it(`keeps visible ${kind} still through metadata and row replacement (native anchoring ${nativeAnchoring})`, () => {
        const rows = [{ key: "before", height: 200 }, { key: "reading", height: 1000, inset: 100 }, { key: "after", height: 200 }];
        const { scroller } = conversation(rows);
        const measure = Element.prototype.getBoundingClientRect;
        Element.prototype.getBoundingClientRect = function () {
          if (kind === "attachment sibling" && this.matches(".task-comment-body")) return { top: -600, bottom: -500 };
          if (this.matches("[data-reading-leaf], .task-comment-body")) {
            const box = measure.call(this);
            return { ...box, top: box.top + rows[1].inset };
          }
          return measure.call(this);
        };
        const paint = () => { scroller.innerHTML = `<div data-key="reading"><div class="task-comment-body">${content}</div></div>`; };
        paint();
        scroller.scrollTop = 600;
        const top = () => scroller.querySelector("[data-reading-leaf]").getBoundingClientRect().top;
        const before = top();
        paintKeepingPlace(scroller, () => {
          rows[1].inset += 100; rows[1].height += 100;
          paint(); // a new row with the same key must recover its content anchor
          if (nativeAnchoring) scroller.scrollTop += 100;
        }, { opening: () => false, policy: followConversation() });
        expect(top()).toBe(before);
        expect(scroller.scrollTop).toBe(700);
      });
    }

    it("keeps the first visible formatted paragraph when only a later paragraph moves", () => {
      const { scroller } = conversation([{ key: "reading", height: 1500 }]);
      scroller.innerHTML = '<div data-key="reading"><div class="task-comment-body"><p data-first><strong>First visible text</strong></p><p data-later>Later plain text</p></div></div>';
      scroller.scrollTop = 600;
      const measure = Element.prototype.getBoundingClientRect;
      let laterGrowth = 0;
      Element.prototype.getBoundingClientRect = function () {
        if (this.closest("[data-first]")) return { top: 500 - scroller.scrollTop, bottom: 800 - scroller.scrollTop };
        if (this.matches("[data-later]")) return { top: 800 + laterGrowth - scroller.scrollTop, bottom: 1200 + laterGrowth - scroller.scrollTop };
        return measure.call(this);
      };
      paintKeepingPlace(scroller, () => {
        laterGrowth = 100;
        scroller.querySelector("[data-later]").textContent += " More lower content";
      }, { opening: () => false, policy: followConversation() });
      expect(scroller.scrollTop).toBe(600);
      expect(scroller.querySelector("[data-first]").getBoundingClientRect().top).toBe(-100);
    });

    it("does not apply the same correction twice after native anchoring", () => {
      const rows = [{ key: "before", height: 200 }, { key: "reading", height: 1000 }, { key: "after", height: 200 }];
      const { scroller, paint } = conversation(rows);
      paint();
      scroller.scrollTop = 600;
      const writes = watchScrollTop(scroller);
      paintKeepingPlace(scroller, () => {
        rows[0].height += 100;
        paint();
        scroller.scrollTop += 100; // the browser already kept the entry still
      }, { opening: () => false, policy: followConversation() });
      expect(scroller.scrollTop).toBe(700);
      expect(writes).toEqual([700]);
    });

    it("ignores a navigation button sharing the reading entry's key", () => {
      const rows = [{ key: "before", height: 200 }, { key: "reading", height: 1000 }, { key: "after", height: 200 }];
      const { scroller, paint } = conversation(rows);
      const measure = Element.prototype.getBoundingClientRect;
      Element.prototype.getBoundingClientRect = function () {
        return this.tagName === "BUTTON" ? { top: 0, bottom: 20 } : measure.call(this);
      };
      const paintWithTick = () => {
        paint();
        const tick = document.createElement("button");
        tick.dataset.key = "reading";
        scroller.prepend(tick);
      };
      paintWithTick();
      scroller.scrollTop = 600;
      const top = () => scroller.querySelector('.msg[data-key="reading"]').getBoundingClientRect().top;
      const before = top();
      paintKeepingPlace(scroller, () => { rows[0].height += 100; paintWithTick(); }, { opening: () => false, policy: followConversation() });
      expect(top()).toBe(before);
      expect(scroller.scrollTop).toBe(700);
    });

    it("does not mistake an early programmatic scroll for reader input", () => {
      const { scroller } = conversation([{ key: "a", height: 400 }]);
      const clock = vi.spyOn(performance, "now").mockReturnValue(100);
      try {
        wireReaderMotion(scroller);
        scroller.dispatchEvent(new Event("scroll"));
        expect(readerIsMoving(scroller)).toBe(false);
      } finally { clock.mockRestore(); }
    });

    // On iOS a write to scrollTop during a fling ends the fling where the
    // write said; a paint that lands mid-fling must not write at all.
    it("writes nothing while the reader is moving the list", () => {
      const { scroller, paint } = conversation([
        { key: "a", height: 400 },
        { key: "b", height: 400 },
      ]);
      paintKeepingPlace(scroller, paint, { opening, policy: followUnread() });
      wireReaderMotion(scroller);
      scroller.dispatchEvent(new Event("touchstart"));
      scroller.scrollTop = 780; // within the slack: a follower, mid-fling
      const writes = watchScrollTop(scroller);
      paintKeepingPlace(scroller, paint, { opening: () => false, policy: followUnread() });
      expect(writes).toEqual([]);
      scroller.dispatchEvent(new Event("touchend"));
      expect(readerIsMoving(scroller)).toBe(false);
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
