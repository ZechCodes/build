// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { whenVisible } from "../src/core/visibility.js";

const setHidden = (hidden) =>
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });

describe("whenVisible", () => {
  it("runs the wrapped function while the page is visible", () => {
    setHidden(false);
    const fn = vi.fn();
    whenVisible(fn)("arg");
    expect(fn).toHaveBeenCalledWith("arg");
  });

  it("skips the wrapped function while the page is hidden", () => {
    setHidden(true);
    const fn = vi.fn();
    whenVisible(fn)();
    expect(fn).not.toHaveBeenCalled();
  });

  it("resumes once the page is visible again", () => {
    const fn = vi.fn();
    const gated = whenVisible(fn);
    setHidden(true);
    gated();
    setHidden(false);
    gated();
    expect(fn).toHaveBeenCalledTimes(1);
  });
});
