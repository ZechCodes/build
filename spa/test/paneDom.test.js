// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

// A stand-in for ghostty-web: the pane only needs a Terminal it can open into a
// host, take onData from, and ask about modes.
const terminals = [];
vi.mock("ghostty-web", () => {
  class Terminal {
    constructor() {
      this.cols = 80;
      this.rows = 24;
      this.buffer = { active: { type: "normal" } };
      this.opened = null;
      this.written = [];
      this.dataHandler = null;
      this.resizeHandler = null;
      this.modes = {};
      terminals.push(this);
    }
    open(host) { this.opened = host; }
    loadAddon() {}
    onData(handler) { this.dataHandler = handler; }
    onResize(handler) { this.resizeHandler = handler; }
    getMode(mode) { return !!this.modes[mode]; }
    hasMouseTracking() { return false; }
    attachCustomWheelEventHandler() {}
    resize(cols, rows) { this.cols = cols; this.rows = rows; }
    reset() {}
    write(bytes) { this.written.push(bytes); }
    dispose() {}
  }
  class FitAddon {
    fit() {}
    proposeDimensions() { return { cols: 80, rows: 24 }; }
  }
  return { init: async () => {}, Terminal, FitAddon };
});

const { mountTerminalPane } = await import("../src/terminal/pane.js");

const press = (element) => element.dispatchEvent(new window.Event("pointerdown", { bubbles: true, cancelable: true }));

async function mount({ coarse = true } = {}) {
  const host = document.createElement("div");
  host.className = "termpane";
  document.body.appendChild(host);
  const sent = [];
  const pane = await mountTerminalPane(host, {
    attach: async () => {},
    input: async (data) => { sent.push(data); },
    resize: async () => {},
    isTouchDevice: () => coarse,
  });
  return { host, pane, sent, term: terminals[terminals.length - 1] };
}

beforeEach(() => {
  terminals.length = 0;
  document.body.innerHTML = "";
  window.ResizeObserver = class { observe() {} disconnect() {} };
});

afterEach(() => {
  delete window.ResizeObserver;
});

describe("mountTerminalPane on a touch device", () => {
  it("gives the terminal its own box so the key bar is a sibling, not an overlay", async () => {
    const { host, term } = await mount();
    const screen = host.querySelector(".term-screen");
    expect(screen).toBeTruthy();
    expect(term.opened).toBe(screen);
    expect(screen.style.touchAction).toBe("none");
    expect(host.querySelector(".termkeys").previousElementSibling).toBe(screen);
  });

  it("sends a bar key straight to the PTY", async () => {
    const { host, sent } = await mount();
    press(host.querySelector('[data-key="esc"]'));
    expect(sent).toEqual(["\x1b"]);
  });

  it("folds a modifier armed on the bar into the next soft-keyboard keystroke", async () => {
    const { host, sent, term } = await mount();
    press(host.querySelector('[data-key="ctrl"]'));
    term.dataHandler("c");
    expect(sent).toEqual(["\x03"]);
    // …and releases it, so the keystroke after that is plain again.
    term.dataHandler("c");
    expect(sent).toEqual(["\x03", "c"]);
  });

  it("keeps a locked modifier folding into keystroke after keystroke", async () => {
    const { host, sent, term } = await mount();
    const alt = host.querySelector('[data-key="alt"]');
    press(alt);
    press(alt);
    term.dataHandler("b");
    term.dataHandler("f");
    expect(sent).toEqual(["\x1bb", "\x1bf"]);
  });

  it("follows the terminal into application cursor mode", async () => {
    const { host, sent, term } = await mount();
    term.modes[1] = true;
    press(host.querySelector('[data-key="up"]'));
    expect(sent).toEqual(["\x1bOA"]);
  });

  it("dispose() takes the bar with it", async () => {
    const { host, pane } = await mount();
    pane.dispose();
    expect(host.querySelector(".termkeys")).toBe(null);
  });
});

describe("mountTerminalPane on a pointer device", () => {
  it("mounts no key bar", async () => {
    const { host } = await mount({ coarse: false });
    expect(host.querySelector(".termkeys")).toBe(null);
    expect(host.querySelector(".term-screen")).toBeTruthy();
  });

  it("passes keystrokes through untouched", async () => {
    const { sent, term } = await mount({ coarse: false });
    term.dataHandler("c");
    expect(sent).toEqual(["c"]);
  });
});
