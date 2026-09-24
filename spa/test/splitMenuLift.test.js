// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { menuButtonMarkup, mountSplitMenu } from "../src/core/splitButton.js";
import { motionBeat, recordAnimations, settleMotion, stopRecordingAnimations } from "./motionRecorder.js";

const OPTIONS = [
  { id: "stop", menuLabel: "Ask to stop", description: "Ask the agent to stop" },
  { id: "output", menuLabel: "Ask for its output", description: "Ask what it printed" },
];

const box = ({ top, bottom, left, right }) => ({ top, bottom, left, right, width: right - left, height: bottom - top, x: left, y: top });

function mountMenuInside(host) {
  const container = document.createElement("div");
  container.innerHTML = menuButtonMarkup("Ask", OPTIONS);
  host.appendChild(container);
  const handle = mountSplitMenu(container, { onChoose: () => {} });
  return { container, ...handle, caret: container.querySelector(".caret"), menu: container.querySelector(".splitmenu") };
}

function scrollingHost() {
  const host = document.createElement("div");
  host.style.overflowY = "auto";
  document.body.appendChild(host);
  return host;
}

describe("a split menu that opens inside a scrolling container", () => {
  beforeEach(() => {
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    Object.defineProperty(window, "innerWidth", { value: 1200, configurable: true });
  });
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("is lifted to fixed positioning below its button, from the button's own box", () => {
    const { container, caret, menu } = mountMenuInside(scrollingHost());
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 500, bottom: 530, left: 900, right: 980 });
    Object.defineProperty(menu, "offsetHeight", { value: 90, configurable: true });
    Object.defineProperty(menu, "offsetWidth", { value: 180, configurable: true });

    caret.click();

    expect(menu.hidden).toBe(false);
    expect(menu.style.position).toBe("fixed");
    expect(menu.style.top).toBe("536px");
    expect(menu.style.left).toBe("800px");
    expect(menu.style.right).toBe("auto");
    expect(menu.style.bottom).toBe("auto");
  });

  it("opens below the button when there is no room above it", () => {
    const { container, caret, menu } = mountMenuInside(scrollingHost());
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 40, bottom: 70, left: 900, right: 980 });
    Object.defineProperty(menu, "offsetHeight", { value: 90, configurable: true });
    Object.defineProperty(menu, "offsetWidth", { value: 180, configurable: true });

    caret.click();

    expect(menu.style.position).toBe("fixed");
    expect(menu.style.top).toBe("76px");
    expect(menu.style.left).toBe("800px");
    expect(menu.style.right).toBe("auto");
    expect(menu.style.bottom).toBe("auto");
  });

  it("flips above the button only when the menu does not fit below it", () => {
    const { container, caret, menu } = mountMenuInside(scrollingHost());
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 700, bottom: 730, left: 900, right: 980 });
    Object.defineProperty(menu, "offsetHeight", { value: 90, configurable: true });
    Object.defineProperty(menu, "offsetWidth", { value: 180, configurable: true });

    caret.click();

    expect(menu.style.position).toBe("fixed");
    expect(menu.style.bottom).toBe("106px");
    expect(menu.style.top).toBe("auto");
  });

  it("holds a menu that fits on neither side inside the viewport", () => {
    Object.defineProperty(window, "innerHeight", { value: 200, configurable: true });
    const { container, caret, menu } = mountMenuInside(scrollingHost());
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 90, bottom: 120, left: 900, right: 980 });
    Object.defineProperty(menu, "offsetHeight", { value: 300, configurable: true });
    Object.defineProperty(menu, "offsetWidth", { value: 180, configurable: true });

    caret.click();

    expect(menu.style.top).toBe("8px");
    expect(menu.style.bottom).toBe("auto");
  });

  it("never hangs a head-height menu off the top of a narrow viewport", () => {
    Object.defineProperty(window, "innerWidth", { value: 420, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 720, configurable: true });
    const { container, caret, menu } = mountMenuInside(scrollingHost());
    // The conversation head's ⋮, where the bug was reported: a button at the
    // top of the panel, on a phone-width viewport.
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 56, bottom: 84, left: 380, right: 408 });
    Object.defineProperty(menu, "offsetHeight", { value: 140, configurable: true });
    Object.defineProperty(menu, "offsetWidth", { value: 220, configurable: true });

    caret.click();

    expect(Number.parseFloat(menu.style.top)).toBeGreaterThanOrEqual(8);
    expect(Number.parseFloat(menu.style.top) + 140).toBeLessThanOrEqual(720);
    expect(Number.parseFloat(menu.style.left)).toBeGreaterThanOrEqual(8);
    expect(Number.parseFloat(menu.style.left) + 220).toBeLessThanOrEqual(420);
    expect(menu.style.bottom).toBe("auto");
  });

  it("clamps a wide menu inside the viewport gutter", () => {
    const { container, caret, menu } = mountMenuInside(scrollingHost());
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 300, bottom: 330, left: 10, right: 100 });
    Object.defineProperties(menu, {
      offsetHeight: { value: 90, configurable: true },
      offsetWidth: { value: 180, configurable: true },
    });

    caret.click();

    expect(menu.style.left).toBe("8px");
    expect(menu.style.right).toBe("auto");
  });

  it("closes when the container scrolls, and takes its inline placement with it", async () => {
    const host = scrollingHost();
    const { container, caret, menu } = mountMenuInside(host);
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 500, bottom: 530, left: 900, right: 980 });
    caret.click();
    expect(menu.hidden).toBe(false);

    host.dispatchEvent(new Event("scroll", { bubbles: false }));
    await motionBeat();

    expect(menu.hidden).toBe(true);
    expect(menu.style.position).toBe("");
    expect(menu.style.left).toBe("");
    expect(menu.style.bottom).toBe("");
    expect(menu.style.right).toBe("");
  });

  it("closes on a viewport resize", () => {
    const { container, caret, menu } = mountMenuInside(scrollingHost());
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 500, bottom: 530, left: 900, right: 980 });
    caret.click();

    window.dispatchEvent(new Event("resize"));

    expect(menu.hidden).toBe(true);
  });

  it("stops listening for scrolls once closed by a choice", async () => {
    const host = scrollingHost();
    const removeListener = vi.spyOn(document, "removeEventListener");
    const { container, caret, menu } = mountMenuInside(host);
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 500, bottom: 530, left: 900, right: 980 });
    caret.click();

    menu.querySelector('.mi[data-action="stop"]').click();
    await motionBeat();

    expect(menu.hidden).toBe(true);
    expect(removeListener.mock.calls.some(([type]) => type === "scroll")).toBe(true);
    removeListener.mockRestore();
  });
});

describe("a split menu with no scrolling container above it", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("keeps the stylesheet's absolute placement", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const { caret, menu } = mountMenuInside(host);

    caret.click();

    expect(menu.hidden).toBe(false);
    expect(menu.style.position).toBe("");
    expect(menu.style.bottom).toBe("");
  });
});

describe("a split menu's motion", () => {
  let started = [];

  beforeEach(() => {
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    Object.defineProperty(window, "innerWidth", { value: 1200, configurable: true });
    started = recordAnimations();
  });
  afterEach(async () => {
    await settleMotion();
    stopRecordingAnimations();
    document.body.innerHTML = "";
  });

  const plainHost = () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    return host;
  };
  const keyframedProperties = (run) => Object.keys(run.keyframes[0]);

  it("grows the menu open on the caret, and shrinks it shut on the next press", async () => {
    const { caret, menu } = mountMenuInside(plainHost());

    caret.click();
    await motionBeat();

    expect(started).toHaveLength(1);
    expect(started[0].element).toBe(menu);
    expect(keyframedProperties(started[0])).toContain("height");
    expect(menu.hidden).toBe(false);

    await settleMotion();
    started.length = 0;
    caret.click();
    await motionBeat();

    expect(started).toHaveLength(1);
    expect(keyframedProperties(started[0])).toContain("height");
    expect(menu.hidden).toBe(false);

    await settleMotion();
    expect(menu.hidden).toBe(true);
  });

  it("shrinks it shut the same way on a press outside it", async () => {
    const { menu, caret } = mountMenuInside(plainHost());
    caret.click();
    await settleMotion();
    started.length = 0;

    document.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    await motionBeat();

    expect(started.filter((run) => run.element === menu)).toHaveLength(1);
    expect(menu.hidden).toBe(false);

    await settleMotion();
    expect(menu.hidden).toBe(true);
  });

  it("holds a lifted menu where it stands until it has finished shrinking", async () => {
    const { container, caret, menu } = mountMenuInside(scrollingHost());
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 700, bottom: 730, left: 900, right: 980 });
    Object.defineProperty(menu, "offsetHeight", { value: 90, configurable: true });

    caret.click();
    await settleMotion();
    expect(menu.style.position).toBe("fixed");
    started.length = 0;

    caret.click();
    await motionBeat();

    expect(menu.style.position).toBe("fixed");
    expect(menu.style.bottom).toBe("106px");

    await settleMotion();

    expect(menu.hidden).toBe(true);
    expect(menu.style.position).toBe("");
    expect(menu.style.bottom).toBe("");
  });

  it("keeps the placement a lifted menu is wearing when it opens again mid-shrink", async () => {
    const { container, caret, menu } = mountMenuInside(scrollingHost());
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 700, bottom: 730, left: 900, right: 980 });
    Object.defineProperty(menu, "offsetHeight", { value: 90, configurable: true });
    caret.click();
    await settleMotion();

    const addListener = vi.spyOn(document, "addEventListener");

    caret.click();
    await motionBeat();
    caret.click();
    await settleMotion();

    expect(menu.hidden).toBe(false);
    expect(menu.style.position).toBe("fixed");
    expect(menu.style.bottom).toBe("106px");
    expect(addListener.mock.calls.filter(([type]) => type === "scroll")).toHaveLength(0);
    addListener.mockRestore();
  });

  it("opens again on a caret press that lands while it is shutting", async () => {
    const { caret, menu } = mountMenuInside(plainHost());
    caret.click();
    await settleMotion();
    started.length = 0;

    caret.click();
    await motionBeat();
    expect(started).toHaveLength(1);

    caret.click();
    await settleMotion();

    expect(started[0].cancelled).toBe(true);
    expect(started).toHaveLength(2);
    expect(menu.hidden).toBe(false);
  });
});

/// The region a menu is kept within. The conversation panel stops on the
/// bubble strip at phone width and the strip is drawn over it, so a menu run
/// to the viewport's foot had its last rows under the strip (#124). The rail
/// names its panel; the menu stays above that box's bottom edge, and grows a
/// scrollbar rather than a tail when even that is not enough room.
describe("a lifted menu kept within a region", () => {
  beforeEach(() => {
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    Object.defineProperty(window, "innerWidth", { value: 1200, configurable: true });
  });
  afterEach(() => {
    document.body.innerHTML = "";
  });

  const mountWithin = (regionBox) => {
    const host = scrollingHost();
    const region = document.createElement("div");
    region.getBoundingClientRect = () => box(regionBox);
    host.appendChild(region);
    const container = document.createElement("div");
    container.innerHTML = menuButtonMarkup("Ask", OPTIONS);
    region.appendChild(container);
    const handle = mountSplitMenu(container, { onChoose: () => {}, keepWithin: () => region });
    return { container, ...handle, caret: container.querySelector(".caret"), menu: container.querySelector(".splitmenu") };
  };

  it("stops at the region's bottom edge rather than the viewport's", () => {
    const { container, caret, menu } = mountWithin({ top: 0, bottom: 600, left: 0, right: 1200 });
    // A head near the top: no room above, and below it the region's edge
    // comes before the viewport's. Unbounded, this menu would open at 136.
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 100, bottom: 130, left: 900, right: 980 });
    Object.defineProperty(menu, "offsetHeight", { value: 470, configurable: true });
    Object.defineProperty(menu, "offsetWidth", { value: 180, configurable: true });

    caret.click();

    expect(menu.style.top).toBe("122px");
    expect(menu.style.bottom).toBe("auto");
  });

  it("scrolls inside the room the region leaves rather than running off it", async () => {
    const { container, caret, menu } = mountWithin({ top: 0, bottom: 600, left: 0, right: 1200 });
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 40, bottom: 70, left: 900, right: 980 });
    Object.defineProperty(menu, "offsetHeight", { value: 900, configurable: true });
    Object.defineProperty(menu, "offsetWidth", { value: 180, configurable: true });

    caret.click();
    expect(menu.style.maxHeight).toBe("584px");

    menu.querySelector('.mi[data-action="stop"]').click();
    await motionBeat();

    expect(menu.style.maxHeight).toBe("");
  });

  it("does not shut on a scroll inside itself", () => {
    const { container, caret, menu } = mountWithin({ top: 0, bottom: 600, left: 0, right: 1200 });
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 40, bottom: 70, left: 900, right: 980 });
    caret.click();

    menu.dispatchEvent(new Event("scroll", { bubbles: false }));

    expect(menu.hidden).toBe(false);
  });

  it("falls back to the viewport when the region reports no box", () => {
    const { container, caret, menu } = mountWithin({ top: 0, bottom: 0, left: 0, right: 0 });
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 500, bottom: 530, left: 900, right: 980 });
    Object.defineProperty(menu, "offsetHeight", { value: 90, configurable: true });
    Object.defineProperty(menu, "offsetWidth", { value: 180, configurable: true });

    caret.click();

    expect(menu.style.top).toBe("536px");
  });
});
