// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountSurfaceClearance } from "../src/core/agentSurfaces.js";

let resize;
let observed;

beforeEach(() => {
  observed = null;
  globalThis.ResizeObserver = class {
    constructor(callback) { resize = callback; }
    observe(element) { observed = element; }
    disconnect = vi.fn();
  };
  document.body.innerHTML = `<section class="rail-panel">
    <div class="rail-body" id="rail-body"></div>
    <div class="rail-composer"><div class="rail-surfaces-viewer"></div></div>
  </section>`;
});

afterEach(() => {
  delete globalThis.ResizeObserver;
  document.body.innerHTML = "";
});

const geometry = ({ scrollTop, height = 180 }) => {
  const scroller = document.querySelector("#rail-body");
  const viewer = document.querySelector(".rail-surfaces-viewer");
  let clearance = 0;
  Object.defineProperties(scroller, {
    clientHeight: { get: () => 300 },
    scrollHeight: { get: () => 900 + clearance },
  });
  scroller.scrollTop = scrollTop;
  viewer.getBoundingClientRect = () => ({ height });
  vi.spyOn(window, "getComputedStyle").mockReturnValue({ marginBottom: "9px" });
  const nativeSet = scroller.style.setProperty.bind(scroller.style);
  vi.spyOn(scroller.style, "setProperty").mockImplementation((name, value) => {
    nativeSet(name, value);
    if (name === "--surface-popover-clearance") clearance = Number.parseFloat(value);
  });
  return { scroller, viewer };
};

describe("the activity popover's conversation clearance", () => {
  it("adds its measured height to the scroller and keeps a reader at the bottom through resizes", () => {
    const { scroller, viewer } = geometry({ scrollTop: 600 });
    const dispose = mountSurfaceClearance(viewer);

    expect(observed).toBe(viewer);
    expect(scroller.style.getPropertyValue("--surface-popover-clearance")).toBe("189px");
    expect(scroller.scrollTop).toBe(1089);

    viewer.getBoundingClientRect = () => ({ height: 240 });
    resize();
    expect(scroller.style.getPropertyValue("--surface-popover-clearance")).toBe("249px");
    expect(scroller.scrollTop).toBe(1149);

    dispose();
    expect(scroller.style.getPropertyValue("--surface-popover-clearance")).toBe("");
  });

  it("preserves a reader's position when the popover opens or changes size", () => {
    const { scroller, viewer } = geometry({ scrollTop: 120 });
    mountSurfaceClearance(viewer);
    expect(scroller.scrollTop).toBe(120);

    viewer.getBoundingClientRect = () => ({ height: 260 });
    resize();
    expect(scroller.scrollTop).toBe(120);

    viewer.hidden = true;
    resize();
    expect(scroller.style.getPropertyValue("--surface-popover-clearance")).toBe("0px");
    expect(scroller.scrollTop).toBe(120);
  });
});
