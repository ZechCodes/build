// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { createFileViewerState, fileViewerModes } from "../src/core/fileViewer.js";
import { mountFileEditor } from "../src/core/fileEditor.js";

const file = (overrides = {}) => ({ mime: "text/markdown", truncated: false, editable: true, revision: "r1", ...overrides });

describe("file viewer modes", () => {
  it("offers Preview, Source and Edit for complete editable rendered text", () => {
    expect(fileViewerModes(file())).toEqual(["preview", "source", "edit"]);
  });

  it("offers Source and Edit for plain text, and no tray for binary or truncated files", () => {
    expect(fileViewerModes(file({ mime: "text/plain" }))).toEqual(["source", "edit"]);
    expect(fileViewerModes(file({ mime: "application/octet-stream", editable: false }))).toEqual([]);
    expect(fileViewerModes(file({ truncated: true, editable: false }))).toEqual(["preview", "source"]);
  });

  it("preserves rendered and source views from an older bridge while withholding Edit", () => {
    expect(fileViewerModes({ mime: "text/markdown", truncated: false })).toEqual(["preview", "source"]);
  });

  it("keeps an edit buffer and its dirty state while modes change", () => {
    const state = createFileViewerState({ file: file(), text: "one\n" });
    state.edit("two\n", { start: 2, end: 2 });
    state.choose("preview");
    state.choose("edit");
    expect(state.snapshot()).toMatchObject({ mode: "edit", value: "two\n", dirty: true, selection: { start: 2, end: 2 } });
  });
});

describe("the native file editor", () => {
  it("reports edits and restores selection", () => {
    const host = document.createElement("div");
    const edits = [];
    const editor = mountFileEditor(host, { value: "hello", selection: { start: 1, end: 3 }, onEdit: (...args) => edits.push(args) });
    const input = host.querySelector("textarea");
    expect([input.selectionStart, input.selectionEnd]).toEqual([1, 3]);
    input.value = "hello!";
    input.setSelectionRange(6, 6);
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(edits.at(-1)).toEqual(["hello!", { start: 6, end: 6 }]);
    editor.dispose();
  });
});
