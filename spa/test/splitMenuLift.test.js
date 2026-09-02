// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { menuButtonMarkup, mountSplitMenu } from "../src/core/splitButton.js";

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

  it("is lifted to fixed positioning above its button, from the button's own box", () => {
    const { container, caret, menu } = mountMenuInside(scrollingHost());
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 500, bottom: 530, left: 900, right: 980 });
    Object.defineProperty(menu, "offsetHeight", { value: 90, configurable: true });

    caret.click();

    expect(menu.hidden).toBe(false);
    expect(menu.style.position).toBe("fixed");
    expect(menu.style.bottom).toBe("306px");
    expect(menu.style.right).toBe("220px");
    expect(menu.style.top).toBe("");
  });

  it("opens below the button when there is no room above it", () => {
    const { container, caret, menu } = mountMenuInside(scrollingHost());
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 40, bottom: 70, left: 900, right: 980 });
    Object.defineProperty(menu, "offsetHeight", { value: 90, configurable: true });

    caret.click();

    expect(menu.style.position).toBe("fixed");
    expect(menu.style.top).toBe("76px");
    expect(menu.style.right).toBe("220px");
    expect(menu.style.bottom).toBe("");
  });

  it("closes when the container scrolls, and takes its inline placement with it", () => {
    const host = scrollingHost();
    const { container, caret, menu } = mountMenuInside(host);
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 500, bottom: 530, left: 900, right: 980 });
    caret.click();
    expect(menu.hidden).toBe(false);

    host.dispatchEvent(new Event("scroll", { bubbles: false }));

    expect(menu.hidden).toBe(true);
    expect(menu.style.position).toBe("");
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

  it("stops listening for scrolls once closed by a choice", () => {
    const host = scrollingHost();
    const removeListener = vi.spyOn(document, "removeEventListener");
    const { container, caret, menu } = mountMenuInside(host);
    container.querySelector(".splitbtn").getBoundingClientRect = () => box({ top: 500, bottom: 530, left: 900, right: 980 });
    caret.click();

    menu.querySelector('.mi[data-action="stop"]').click();

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
