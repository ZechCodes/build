// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import {
  createViewingContext,
  viewingContextChipsHtml,
} from "../src/core/viewingContext.js";

describe("viewing context", () => {
  it("freezes a bounded deep snapshot and keeps the artifact when selection clears", () => {
    const context = createViewingContext();
    context.set({ kind: "file", path: "src/app.js" });
    context.setSelection([{ kind: "selection", path: "src/app.js", text: "hello", line_start: 2, line_end: 2 }]);
    const snapshot = context.snapshot();
    context.clearSelection();

    expect(snapshot).toEqual({ version: 1, items: [
      { kind: "file", path: "src/app.js" },
      { kind: "selection", path: "src/app.js", text: "hello", line_start: 2, line_end: 2 },
    ] });
    expect(context.snapshot()).toEqual({ version: 1, items: [{ kind: "file", path: "src/app.js" }] });
  });

  it("reports only file elements clipped into the real scroller viewport", () => {
    const scroller = document.createElement("div");
    Object.defineProperty(scroller, "clientHeight", { value: 100 });
    scroller.getBoundingClientRect = () => ({ top: 20, bottom: 120, left: 0, right: 100 });
    for (const [key, top, bottom] of [["a.js", -500, -400], ["b.js", 30, 80], ["c.js", 150, 220]]) {
      const file = document.createElement("div");
      file.className = "file";
      file.dataset.key = key;
      file.getBoundingClientRect = () => ({ top, bottom, left: 0, right: 100 });
      scroller.append(file);
    }
    const context = createViewingContext();
    context.setVisibleDiffs(scroller, "uncommitted");
    expect(context.snapshot().items).toEqual([{ kind: "diff", path: "b.js", mode: "uncommitted" }]);
  });

  it("clips visible diffs to the browser viewport and overflow ancestors", () => {
    const clip = document.createElement("div");
    clip.style.overflow = "hidden";
    clip.getBoundingClientRect = () => ({ top: 40, bottom: 70, left: 0, right: 100 });
    const scroller = document.createElement("div");
    scroller.getBoundingClientRect = () => ({ top: 0, bottom: 200, left: 0, right: 100 });
    clip.append(scroller);
    document.body.append(clip);
    for (const [path, top, bottom] of [["above.js", 10, 30], ["shown.js", 45, 60], ["below.js", 80, 100]]) {
      const file = document.createElement("div");
      file.className = "file";
      file.dataset.key = path;
      file.getBoundingClientRect = () => ({ top, bottom, left: 0, right: 100 });
      scroller.append(file);
    }
    const context = createViewingContext();
    context.setVisibleDiffs(scroller, "all");
    expect(context.snapshot().items).toEqual([{ kind: "diff", path: "shown.js", mode: "all" }]);
  });

  it("reports no files when clipping produces an empty rectangle", () => {
    const clip = document.createElement("div");
    clip.style.overflow = "hidden";
    clip.getBoundingClientRect = () => ({ top: 300, bottom: 400, left: 0, right: 100 });
    const scroller = document.createElement("div");
    scroller.getBoundingClientRect = () => ({ top: 0, bottom: 200, left: 0, right: 100 });
    const file = document.createElement("div");
    file.className = "file";
    file.dataset.key = "spanning.js";
    file.getBoundingClientRect = () => ({ top: 0, bottom: 500, left: 0, right: 100 });
    scroller.append(file);
    clip.append(scroller);
    document.body.append(clip);
    const context = createViewingContext();
    context.setVisibleDiffs(scroller, "all");
    expect(context.snapshot()).toBeUndefined();
  });

  it("clears the sent visible excerpt after another excerpt was removed", () => {
    const context = createViewingContext();
    context.setSelection([
      { kind: "selection", path: "a.js", text: "a" },
      { kind: "selection", path: "b.js", text: "b" },
    ]);
    context.remove(0);
    const sent = context.snapshot();
    context.clearSelectionIfMatches(sent);
    expect(context.snapshot()).toBeUndefined();
  });

  it("captures diff selections without line-number gutters and splits paths and sides", () => {
    document.body.innerHTML = `<div id="diff"><div class="file" data-key="a.js"><table><tbody>
      <tr data-side="old" data-old-line="3"><td class="ln">3</td><td class="ln"></td><td class="code">old line</td></tr>
      <tr data-side="new" data-new-line="4"><td class="ln"></td><td class="ln">4</td><td class="code">new line</td></tr>
    </tbody></table></div></div>`;
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(document.querySelector("tbody"));
    selection.addRange(range);
    const context = createViewingContext();
    context.captureDomSelection(document.querySelector("#diff"));
    expect(context.snapshot().items).toEqual([
      { kind: "selection", path: "a.js", text: "old line", line_start: 3, line_end: 3, side: "old" },
      { kind: "selection", path: "a.js", text: "new line", line_start: 4, line_end: 4, side: "new" },
    ]);
  });

  it("caps excerpt bytes and marks chips as truncated", () => {
    const context = createViewingContext({ maxSelectionBytes: 5 });
    context.setSelection([{ kind: "selection", path: "a.js", text: "abcdef" }]);
    const snapshot = context.snapshot();
    expect(snapshot.items[0]).toMatchObject({ text: "abcde", truncated: true });
    expect(viewingContextChipsHtml(snapshot)).toContain("Truncated");
    expect(viewingContextChipsHtml(snapshot)).toContain("abcde");
  });

  it("never splits a UTF-16 surrogate pair while truncating UTF-8", () => {
    const context = createViewingContext({ maxSelectionBytes: 4 });
    context.setSelection([{ kind: "selection", path: "a.js", text: "😀x" }]);
    expect(context.snapshot().items[0].text).toBe("😀");
  });
});
