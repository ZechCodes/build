// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
import { mountMeasuredHeight } from "../src/core/measuredInset.js";

afterEach(() => vi.unstubAllGlobals());

describe("measured surface chrome", () => {
  it("tracks wrapping height and removes its inset when the surface leaves", () => {
    let resize = null;
    const disconnect = vi.fn();
    vi.stubGlobal("ResizeObserver", class {
      constructor(callback) { resize = callback; }
      observe() {}
      disconnect() { disconnect(); }
    });
    const chrome = document.createElement("div");
    const surface = document.createElement("div");
    let height = 46;
    chrome.getBoundingClientRect = () => ({ height });

    const dispose = mountMeasuredHeight(chrome, surface, "--chrome-height");
    expect(surface.style.getPropertyValue("--chrome-height")).toBe("46px");
    height = 78;
    resize();
    expect(surface.style.getPropertyValue("--chrome-height")).toBe("78px");

    dispose();
    expect(disconnect).toHaveBeenCalledOnce();
    expect(surface.style.getPropertyValue("--chrome-height")).toBe("");
  });
});
