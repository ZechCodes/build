// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { createFileViewerState, fileBodyReading, fileViewerModes, sameFile } from "../src/core/fileViewer.js";
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

  // A file too large for one record holds pages, not the file (#95): no
  // revision to save against, and no whole document to render.
  it("offers a paged file for reading only, and its markdown as source", () => {
    const paged = (mime) => ({ mime, truncated: false, editable: false, paged: true, of: "v1", size: 5_000_000 });
    expect(fileViewerModes(paged("text/markdown"))).toEqual(["source"]);
    expect(fileViewerModes(paged("text/plain"))).toEqual(["source"]);
    expect(fileViewerModes(paged("text/html"))).toEqual(["preview", "source"]);
    expect(fileViewerModes({ ...paged("text/plain"), editable: true, revision: "r1" })).not.toContain("edit");
  });

  it("names how much of a file each kind of view needs", () => {
    expect(fileBodyReading("application/pdf")).toBe("none");
    expect(fileBodyReading("application/octet-stream")).toBe("none");
    expect(fileBodyReading("image/png")).toBe("media");
    expect(fileBodyReading("video/mp4")).toBe("media");
    expect(fileBodyReading("image/svg+xml")).toBe("rendered");
    expect(fileBodyReading("text/html")).toBe("rendered");
    expect(fileBodyReading("text/markdown")).toBe("lines");
    expect(fileBodyReading(null)).toBe("lines");
  });

  it("tells two paged records apart by the version their pages are of", () => {
    const head = (of) => ({ mime: "text/plain", paged: true, of, truncated: false });
    expect(sameFile(head("v1"), head("v1"))).toBe(true);
    expect(sameFile(head("v1"), head("v2"))).toBe(false);
  });

  it("keeps an edit buffer and its state while modes change", () => {
    const state = createFileViewerState({ file: file(), text: "one\n" });
    state.edit("two\n", { start: 2, end: 2 });
    state.choose("preview");
    state.choose("edit");
    expect(state.snapshot()).toMatchObject({ mode: "edit", value: "two\n", status: "dirty", unsaved: true, selection: { start: 2, end: 2 } });
  });

  it("throws the edit buffer away on revert", () => {
    const state = createFileViewerState({ file: file(), text: "one\n" });
    state.edit("two\n");
    state.revert();
    expect(state.snapshot()).toMatchObject({ value: "one\n", status: "clean", unsaved: false });
  });
});

describe("the draft's lifecycle", () => {
  const at = (revision) => file({ revision });
  const draft = (value = "one\n") => {
    const state = createFileViewerState({ file: at("r1"), text: "one\n" });
    state.edit(value);
    return state;
  };

  it("sends one save at a time, against the baseline's revision", () => {
    const state = draft("two\n");
    expect(state.submit()).toEqual({ value: "two\n", revision: "r1" });
    expect(state.snapshot()).toMatchObject({ status: "saving", submittedRevision: "r1", unsaved: true });
    expect(state.submit()).toBeNull();
  });

  it("takes the acknowledged write as the baseline, never a record that landed after it", () => {
    const state = draft("two\n");
    state.submit();
    expect(state.recordArrived(at("r3"))).toBe("held");
    state.saveSucceeded(at("r2"));
    expect(state.snapshot()).toMatchObject({ status: "clean", baseRevision: "r2", submittedRevision: null });
    expect(state.snapshot().stale).toMatchObject({ revision: "r3" });
  });

  it("keeps an edit made during the save, even one back to the old baseline", () => {
    const state = draft("two\n");
    state.submit();
    state.edit("one\n");
    expect(state.snapshot()).toMatchObject({ status: "saving-edited", unsaved: true });
    state.saveSucceeded(at("r2"));
    expect(state.snapshot()).toMatchObject({ status: "dirty", value: "one\n", baseRevision: "r2" });
  });

  it("goes back to its edits against the old baseline when the save is refused", () => {
    const state = draft("two\n");
    state.submit();
    state.saveFailed(new Error("revision conflict: the file changed"));
    expect(state.snapshot()).toMatchObject({ status: "dirty", baseRevision: "r1", conflict: true });
    state.submit();
    expect(state.snapshot()).toMatchObject({ error: null, conflict: false });
  });

  it("holds a record over edits, adopts one over a clean draft, and ignores its own revision", () => {
    const state = draft("two\n");
    expect(state.recordArrived(at("r1"))).toBe("same");
    expect(state.recordArrived(at("r9"))).toBe("held");
    expect(state.snapshot().disk).toMatchObject({ revision: "r9" });
    expect(state.recordArrived(at("r1"))).toBe("same");
    expect(state.snapshot().disk).toBeNull();
    state.revert();
    expect(state.recordArrived(at("r9"))).toBe("adopt");
  });

  it("lets go of a save that is out when reverted, and ignores its answer", () => {
    const state = draft("two\n");
    state.submit();
    state.revert();
    expect(state.snapshot()).toMatchObject({ status: "clean", unsaved: false, submittedRevision: null });
    state.saveSucceeded(at("r2"));
    expect(state.snapshot()).toMatchObject({ status: "clean", baseRevision: "r1" });
  });

  it("spends a disk note that named the revision its own save produced", () => {
    const state = draft("two\n");
    state.submit();
    state.recordArrived(at("r2"));
    state.saveSucceeded(at("r2"));
    expect(state.snapshot()).toMatchObject({ status: "clean", disk: null, stale: null });
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
