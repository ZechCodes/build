// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
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


describe("grouped viewing context pills", () => {
  const diffs = ["bridge/Cargo.toml", "bridge/examples/dev_relay.rs", "bridge/src/agent.rs"]
    .map((path) => ({ kind: "diff", path, mode: "all" }));

  it("condenses all files into one expandable pill with one separate remove button", () => {
    const host = document.createElement("div");
    host.innerHTML = viewingContextChipsHtml({ version: 1, items: diffs }, { removable: true });
    expect(host.querySelectorAll(".viewing-context-chip")).toHaveLength(1);
    expect(host.querySelector("summary").textContent).toBe("All changes: bridge/Cargo.toml +2");
    expect([...host.querySelectorAll("li")].map((row) => row.textContent)).toEqual(diffs.map((item) => item.path));
    expect(host.querySelector("details").open).toBe(false);
    expect(host.querySelectorAll("button")).toHaveLength(1);
    expect(host.querySelector("summary button")).toBeNull();
    host.querySelector("summary").click();
    expect(host.querySelector("details").open).toBe(true);
  });

  it("keeps diff modes and selected excerpts distinct and escapes file names", () => {
    const host = document.createElement("div");
    host.innerHTML = viewingContextChipsHtml({ version: 1, items: [
      diffs[0],
      { kind: "selection", path: "bridge/Cargo.toml", text: "selected text" },
      { kind: "diff", path: "<img src=x onerror=alert(1)>", mode: "uncommitted" },
      diffs[1],
    ] });
    expect(host.querySelectorAll(".viewing-context-group")).toHaveLength(2);
    expect(host.querySelector("[data-context-group=diff-all] summary").textContent).toBe("All changes: bridge/Cargo.toml +1");
    expect(host.querySelector("[data-context-group=diff-uncommitted] summary").textContent).toBe("Uncommitted: <img src=x onerror=alert(1)>");
    expect(host.querySelector(".viewing-context-detail pre").textContent).toBe("selected text");
    expect(host.querySelector("img")).toBeNull();
    expect(host.querySelector("button")).toBeNull();
  });

  it("removes the entire group in one update while preserving selected excerpts", () => {
    const context = createViewingContext();
    context.set({ version: 1, items: diffs });
    context.setSelection([{ kind: "selection", path: diffs[0].path, text: "keep this excerpt" }]);
    const listener = vi.fn();
    context.subscribe(listener);
    context.removeMany([0, 1, 2]);
    expect(listener).toHaveBeenCalledTimes(1);
    expect(context.snapshot().items).toEqual([{ kind: "selection", path: diffs[0].path, text: "keep this excerpt" }]);
    const scroller = document.createElement("div");
    scroller.getBoundingClientRect = () => ({ top: 0, bottom: 100, left: 0, right: 100 });
    for (const item of diffs) {
      const file = document.createElement("div");
      file.className = "file";
      file.dataset.key = item.path;
      file.getBoundingClientRect = () => ({ top: 0, bottom: 100, left: 0, right: 100 });
      scroller.append(file);
    }
    context.setVisibleDiffs(scroller, "all");
    expect(context.snapshot().items).toHaveLength(1);
    expect(listener).toHaveBeenCalledTimes(1);
  });
});
