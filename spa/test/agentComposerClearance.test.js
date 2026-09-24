// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { mountComposerClearance } from "../src/core/composerClearance.js";

let resize;
globalThis.ResizeObserver = class {
  constructor(callback) { resize = callback; }
  observe() {}
  disconnect() {}
};

const mount = ({ scrollTop, scrollHeight = 1000, clientHeight = 400 }) => {
  document.body.innerHTML = '<div id="panel"><div id="rail-body"></div><div id="rail-footer"></div></div>';
  const panel = document.querySelector("#panel");
  const body = panel.querySelector("#rail-body");
  const footer = panel.querySelector("#rail-footer");
  Object.defineProperties(body, {
    scrollHeight: { configurable: true, get: () => scrollHeight },
    clientHeight: { configurable: true, get: () => clientHeight },
  });
  body.scrollTop = scrollTop;
  footer.getBoundingClientRect = () => ({ height: 120 });
  const dispose = mountComposerClearance(panel);
  return { body, dispose };
};

afterEach(() => { document.body.innerHTML = ""; });

describe("floating composer clearance", () => {
  it("follows a composer resize when the reader is at the bottom", () => {
    const { body } = mount({ scrollTop: 600 });
    expect(body.style.getPropertyValue("--rail-composer-clearance")).toBe("120px");
    resize();
    expect(body.scrollTop).toBe(1000);
  });

  it("keeps an older reading position fixed and cleans up", () => {
    const { body, dispose } = mount({ scrollTop: 250 });
    resize();
    expect(body.scrollTop).toBe(250);
    dispose();
    expect(body.style.getPropertyValue("--rail-composer-clearance")).toBe("");
  });
});
