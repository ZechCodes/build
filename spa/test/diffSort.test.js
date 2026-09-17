import { describe, expect, it, vi } from "vitest";
import {
  DIFF_SORT_ALPHABETICAL,
  DIFF_SORT_LATEST,
  diffSortHtml,
  loadDiffSort,
  saveDiffSort,
  sortDiffFiles,
} from "../src/core/diffSort.js";

describe("diff file sorting", () => {
  const files = [
    { path: "z-last.js", editedAt: 30 },
    { path: "a-first.js", editedAt: 10 },
    { path: "m-middle.js", editedAt: 20 },
  ];

  it("sorts by newest edit or alphabetically without mutating the source", () => {
    expect(sortDiffFiles(files, DIFF_SORT_LATEST).map((file) => file.path)).toEqual([
      "z-last.js",
      "m-middle.js",
      "a-first.js",
    ]);
    expect(sortDiffFiles(files, DIFF_SORT_ALPHABETICAL).map((file) => file.path)).toEqual([
      "a-first.js",
      "m-middle.js",
      "z-last.js",
    ]);
    expect(files.map((file) => file.path)).toEqual(["z-last.js", "a-first.js", "m-middle.js"]);
  });

  it("persists one browser-wide preference and defaults to latest", () => {
    const storage = { getItem: vi.fn(() => null), setItem: vi.fn() };
    expect(loadDiffSort(storage)).toBe(DIFF_SORT_LATEST);
    saveDiffSort(DIFF_SORT_ALPHABETICAL, storage);
    expect(storage.setItem).toHaveBeenCalledWith("build.diffSort", DIFF_SORT_ALPHABETICAL);
    storage.getItem.mockReturnValue(DIFF_SORT_ALPHABETICAL);
    expect(loadDiffSort(storage)).toBe(DIFF_SORT_ALPHABETICAL);
  });

  it("survives browsers that deny access to the localStorage getter", () => {
    const descriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      get: () => {
        throw new DOMException("denied", "SecurityError");
      },
    });
    try {
      expect(loadDiffSort()).toBe(DIFF_SORT_LATEST);
      expect(() => saveDiffSort(DIFF_SORT_ALPHABETICAL)).not.toThrow();
    } finally {
      if (descriptor) Object.defineProperty(globalThis, "localStorage", descriptor);
      else delete globalThis.localStorage;
    }
  });

  it("renders an accessible selector with the current choice", () => {
    const html = diffSortHtml(DIFF_SORT_ALPHABETICAL);
    expect(html).toContain('aria-label="Sort changed files"');
    expect(html).toContain('value="alphabetical" selected');
  });
});
