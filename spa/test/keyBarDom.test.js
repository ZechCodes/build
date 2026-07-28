// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from "vitest";
import { mountKeyBar, coarsePointer } from "../src/terminal/keyBar.js";
import { createStickyModifiers } from "../src/terminal/touchKeys.js";

const bar = () => document.querySelector(".termkeys");
const key = (id) => document.querySelector(`.termkeys [data-key="${id}"]`);
const press = (element) => {
  const event = new window.Event("pointerdown", { bubbles: true, cancelable: true });
  element.dispatchEvent(event);
  return event;
};

function setup(options = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const sent = [];
  const sticky = options.sticky || createStickyModifiers();
  const keyBar = mountKeyBar(host, { send: (data) => sent.push(data), sticky, ...options });
  return { host, sent, sticky, keyBar };
}

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("mountKeyBar", () => {
  it("renders one button per touch key, inside the host", () => {
    const { host } = setup();
    expect(bar().parentElement).toBe(host);
    expect(key("esc").textContent).toBe("Esc");
    expect(key("tab")).toBeTruthy();
    expect(key("ctrl")).toBeTruthy();
    expect(key("up")).toBeTruthy();
  });

  it("sends the key's bytes on pointerdown", () => {
    const { sent } = setup();
    press(key("esc"));
    press(key("tab"));
    expect(sent).toEqual(["\x1b", "\t"]);
  });

  it("suppresses the default action so the soft keyboard keeps its focus", () => {
    setup();
    expect(press(key("esc")).defaultPrevented).toBe(true);
    expect(press(key("ctrl")).defaultPrevented).toBe(true);
  });

  it("suppresses mousedown too — a browser that still fires it would move focus on its own", () => {
    setup();
    const event = new window.MouseEvent("mousedown", { bubbles: true, cancelable: true });
    key("esc").dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    const onBar = new window.MouseEvent("mousedown", { bubbles: true, cancelable: true });
    bar().dispatchEvent(onBar);
    expect(onBar.defaultPrevented).toBe(false);
  });

  it("sends nothing for a press on the bar's own padding", () => {
    const { sent } = setup();
    press(bar());
    expect(sent).toEqual([]);
  });

  it("arms a modifier instead of sending it, and shows the state on the button", () => {
    const { sent, sticky } = setup();
    press(key("ctrl"));
    expect(sent).toEqual([]);
    expect(sticky.state("ctrl")).toBe("armed");
    expect(key("ctrl").classList.contains("armed")).toBe(true);
    expect(key("ctrl").getAttribute("aria-pressed")).toBe("true");
  });

  it("locks a modifier on a second press and marks it distinctly", () => {
    const { sticky } = setup();
    press(key("ctrl"));
    press(key("ctrl"));
    expect(sticky.state("ctrl")).toBe("locked");
    expect(key("ctrl").classList.contains("locked")).toBe(true);
    expect(key("ctrl").classList.contains("armed")).toBe(false);
  });

  it("applies an armed modifier to the next bar key and then releases it", () => {
    const { sent, sticky } = setup();
    press(key("ctrl"));
    press(key("right"));
    expect(sent).toEqual(["\x1b[1;5C"]);
    expect(sticky.state("ctrl")).toBe("off");
    expect(key("ctrl").classList.contains("armed")).toBe(false);
  });

  it("keeps a locked modifier applying to key after key", () => {
    const { sent, sticky } = setup();
    press(key("alt"));
    press(key("alt"));
    press(key("right"));
    press(key("right"));
    expect(sent).toEqual(["\x1b[1;3C", "\x1b[1;3C"]);
    expect(sticky.state("alt")).toBe("locked");
  });

  it("emits application-mode cursor keys when the terminal asks for them", () => {
    const { sent } = setup({ applicationCursor: () => true });
    press(key("up"));
    expect(sent).toEqual(["\x1bOA"]);
  });

  it("repaints modifier buttons when the shared state changes from outside the bar", () => {
    const sticky = createStickyModifiers();
    setup({ sticky });
    sticky.press("ctrl");
    expect(key("ctrl").classList.contains("armed")).toBe(true);
    sticky.consume();
    expect(key("ctrl").classList.contains("armed")).toBe(false);
  });

  it("lifts the bar over an on-screen keyboard the layout viewport ignored", () => {
    window.visualViewport = { height: 460, offsetTop: 0, addEventListener() {}, removeEventListener() {} };
    window.innerHeight = 800;
    setup();
    expect(bar().style.transform).toBe("translateY(-340px)");
    delete window.visualViewport;
  });

  it("sits in flow when nothing covers the viewport", () => {
    const { keyBar } = setup();
    expect(bar().style.transform).toBe("");
    expect(typeof keyBar.dispose).toBe("function");
  });

  it("dispose() removes the bar, stops sending, and clears the modifiers it armed", () => {
    const { keyBar, sent, sticky, host } = setup();
    const ctrl = key("ctrl");
    press(ctrl);
    keyBar.dispose();
    expect(bar()).toBe(null);
    expect(host.children).toHaveLength(0);
    expect(sticky.state("ctrl")).toBe("off");
    press(ctrl);
    expect(sent).toEqual([]);
  });

  it("dispose() leaves a later bar on the same sticky state free to repaint", () => {
    const sticky = createStickyModifiers();
    const first = setup({ sticky });
    first.keyBar.dispose();
    setup({ sticky });
    sticky.press("alt");
    expect(key("alt").classList.contains("armed")).toBe(true);
  });
});

describe("coarsePointer", () => {
  it("is true when the pointer media query matches", () => {
    const win = { matchMedia: vi.fn(() => ({ matches: true })) };
    expect(coarsePointer(win)).toBe(true);
    expect(win.matchMedia).toHaveBeenCalledWith("(pointer: coarse)");
  });

  it("falls back to touch points when the query does not match", () => {
    expect(coarsePointer({ matchMedia: () => ({ matches: false }), navigator: { maxTouchPoints: 5 } })).toBe(true);
    expect(coarsePointer({ matchMedia: () => ({ matches: false }), navigator: { maxTouchPoints: 0 } })).toBe(false);
  });

  it("is false on a plain mouse-driven window", () => {
    expect(coarsePointer({ navigator: { maxTouchPoints: 0 } })).toBe(false);
    expect(coarsePointer({})).toBe(false);
  });
});
