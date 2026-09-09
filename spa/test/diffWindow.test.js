// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { diffFileHtml } from "../src/core/diffRender.js";
import { CAPPED_PREVIEW_ROWS, COLLAPSED_PREVIEW_ROWS, normalizeRowWindow, rowWindowStart } from "../src/core/diffWindow.js";
import { createDiffViewport } from "../src/core/diffViewport.js";

const rows = Array.from({ length: 1000 }, (_, index) => ({ t: "add", n: index + 1, text: `line ${index + 1}` }));
const file = { path: "huge.js", status: "EDIT", add: rows.length, del: 0, rows };

const renderedRows = (html) => (html.match(/<tr class="add"/g) || []).length;

describe("bounded diff rendering", () => {
  it("renders a real capped preview rather than generating hidden rows", () => {
    const html = diffFileHtml(file);
    expect(renderedRows(html)).toBe(CAPPED_PREVIEW_ROWS);
    expect(html).not.toContain("line 1000");
  });

  it("makes shut files a smaller preview", () => {
    expect(renderedRows(diffFileHtml(file, { fold: "shut" }))).toBe(COLLAPSED_PREVIEW_ROWS);
  });

  it("windows an open file while retaining its full scroll extent", () => {
    const viewport = { fileVisible: () => true, rowWindow: () => ({ start: 500 }) };
    const html = diffFileHtml(file, { fold: "open", viewport });
    expect(renderedRows(html)).toBe(240);
    expect(html).toContain('data-ln="501"');
    expect(html).toContain('class="drow-spacer before"');
    expect(html).toContain('class="drow-spacer after"');
    expect(html).toContain('<table aria-rowcount="1000">');
    expect(html).toContain('aria-rowindex="501" data-ln="501"');
  });

  it("gives an offscreen placeholder the same row-derived height as its rendered fold", () => {
    const offscreen = { fileVisible: () => false };
    expect(diffFileHtml(file, { fold: "open", viewport: offscreen })).toContain('style="height:4800px"');
    expect(diffFileHtml(file, { fold: "capped", viewport: offscreen })).toContain('style="height:480px"');
    expect(diffFileHtml(file, { fold: "shut", viewport: offscreen })).toContain('style="height:160px"');

    const short = { ...file, rows: rows.slice(0, 3) };
    expect(diffFileHtml(short, { fold: "open", viewport: offscreen })).toContain('style="height:60px"');
  });

  it("can reach every row through consecutive windows", () => {
    const covered = new Set();
    for (let start = 0; start < rows.length; start += 240) {
      const window = normalizeRowWindow(rows.length, { start });
      for (let index = window.start; index < window.end; index++) covered.add(index);
    }
    expect(covered.size).toBe(rows.length);
  });

  it("moves the row window only at coarse half-window boundaries", () => {
    expect(rowWindowStart(0)).toBe(0);
    expect(rowWindowStart(20)).toBe(0);
    expect(rowWindowStart(3_580)).toBe(0);
    expect(rowWindowStart(3_600)).toBe(120);
    expect(rowWindowStart(3_620)).toBe(120);
  });

  it("keeps absolute hunk marks when the window starts after earlier hunks", () => {
    const markedRows = rows.slice();
    markedRows[0] = { t: "hunk", text: "@@ first @@" };
    markedRows[500] = { t: "hunk", text: "@@ second @@" };
    const marked = { ...file, rows: markedRows, triageHunks: [{ hunk_id: "one", level: "normal" }, { hunk_id: "two", level: "critical" }] };
    const viewport = { fileVisible: () => true, rowWindow: () => ({ start: 500 }) };
    expect(diffFileHtml(marked, { fold: "open", viewport })).toContain('data-hunk="two"');
  });
});

describe("the diff viewport controller", () => {
  const mountedViewport = () => {
    const priorObserver = globalThis.IntersectionObserver;
    const priorAnimationFrame = window.requestAnimationFrame;
    const priorCancelFrame = window.cancelAnimationFrame;
    const frames = [];
    class Observer {
      observe() {}
      disconnect() {}
    }
    globalThis.IntersectionObserver = Observer;
    window.requestAnimationFrame = (callback) => {
      frames.push(callback);
      return frames.length;
    };
    window.cancelAnimationFrame = () => {};
    const scroller = document.createElement("div");
    scroller.innerHTML = '<div class="file" data-key="EDIT:huge.js"><div class="dscroll" data-row-count="1000"></div></div>';
    document.body.appendChild(scroller);
    const restore = () => {
      globalThis.IntersectionObserver = priorObserver;
      window.requestAnimationFrame = priorAnimationFrame;
      window.cancelAnimationFrame = priorCancelFrame;
      scroller.remove();
    };
    return { scroller, box: scroller.querySelector(".dscroll"), flush: () => frames.shift()?.(), restore };
  };

  it("loads only a small initial neighborhood before geometry is attached", () => {
    const viewport = createDiffViewport();
    expect([0, 1, 2, 3].map((index) => viewport.shouldLoad(`file:${index}`, "capped"))).toEqual([true, true, true, false]);
    expect(viewport.shouldLoad("file:0", "shut")).toBe(false);
    viewport.request("file:9");
    expect(viewport.shouldLoad("file:9", "open")).toBe(true);
    viewport.dispose();
  });

  it("records every inner scroll position, including movement within one coarse window", () => {
    const { scroller, box, restore } = mountedViewport();
    const viewport = createDiffViewport();
    try {
      viewport.attach(scroller);
      box.scrollTop = 100;
      box.dispatchEvent(new Event("scroll", { bubbles: true }));
      box.scrollTop = 0;
      viewport.attach(scroller);
      expect(box.scrollTop).toBe(100);
    } finally {
      viewport.dispose();
      restore();
    }
  });

  it("extends a selected window before autoscroll reaches blank spacer rows", () => {
    const { scroller, box, restore } = mountedViewport();
    const viewport = createDiffViewport();
    const priorSelection = document.getSelection;
    try {
      Object.defineProperty(box, "clientHeight", { value: 720 });
      document.getSelection = () => ({ isCollapsed: false, anchorNode: box, focusNode: box });
      viewport.attach(scroller);
      box.scrollTop = 4000;
      box.dispatchEvent(new Event("scroll", { bubbles: true }));
      const extended = viewport.renderOptions().viewport.rowWindow("EDIT:huge.js");
      expect(extended.end).toBeGreaterThan(240);
      box.scrollTop = 4100;
      box.dispatchEvent(new Event("scroll", { bubbles: true }));
      const held = viewport.renderOptions().viewport.rowWindow("EDIT:huge.js");
      expect(held.start).toBe(extended.start);
      expect(held.end).toBeGreaterThanOrEqual(extended.end);

      box.scrollTop = 0;
      box.dispatchEvent(new Event("scroll", { bubbles: true }));
      const reversed = viewport.renderOptions().viewport.rowWindow("EDIT:huge.js");
      expect(reversed.start).toBe(0);
      expect(reversed.end).toBeGreaterThanOrEqual(held.end);
    } finally {
      document.getSelection = priorSelection;
      viewport.dispose();
      restore();
    }
  });

  it("restores native Range endpoints after an upward window prepend", () => {
    const mounted = mountedViewport();
    const { scroller, box, flush, restore } = mounted;
    box.innerHTML = '<table><tbody><tr aria-rowindex="122"><td>selected target</td></tr></tbody></table>';
    const repaint = () => {
      box.innerHTML = '<table><tbody><tr aria-rowindex="1"><td>earlier</td></tr><tr aria-rowindex="122"><td>selected target</td></tr></tbody></table>';
      viewport.attach(scroller);
    };
    const viewport = createDiffViewport({ repaint });
    try {
      Object.defineProperty(box, "clientHeight", { value: 720 });
      viewport.attach(scroller);
      box.scrollTop = 3600;
      box.dispatchEvent(new Event("scroll", { bubbles: true }));
      flush();

      const text = box.querySelector('tr[aria-rowindex="122"] td').firstChild;
      const selection = document.getSelection();
      selection.setBaseAndExtent(text, 0, text, 8);
      box.scrollTop = 2400;
      box.dispatchEvent(new Event("scroll", { bubbles: true }));
      flush();
      expect(document.getSelection().toString()).toBe("selected");
      expect(document.getSelection().anchorNode.parentElement.closest("tr").getAttribute("aria-rowindex")).toBe("122");
    } finally {
      viewport.dispose();
      document.getSelection().removeAllRanges();
      restore();
    }
  });
});
