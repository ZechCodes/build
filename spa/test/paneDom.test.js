// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

// A stand-in for ghostty-web: the pane only needs a Terminal it can open into a
// host, take onData from, and ask about modes.
const terminals = [];
const setup = vi.hoisted(() => ({ fitError: null }));
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
      this.dispose = vi.fn();
      this.dataSubscription = { dispose: vi.fn() };
      this.resizeSubscription = { dispose: vi.fn() };
      terminals.push(this);
    }
    open(host) { this.opened = host; }
    loadAddon() {}
    onData(handler) { this.dataHandler = handler; return this.dataSubscription; }
    onResize(handler) { this.resizeHandler = handler; return this.resizeSubscription; }
    getMode(mode) { return !!this.modes[mode]; }
    hasMouseTracking() { return false; }
    attachCustomWheelEventHandler() {}
    resize(cols, rows) { this.cols = cols; this.rows = rows; }
    reset() {}
    write(bytes) { this.written.push(bytes); }
  }
  class FitAddon {
    fit() { if (setup.fitError) throw setup.fitError; }
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
  setup.fitError = null;
  document.body.innerHTML = "";
  window.ResizeObserver = class { observe() {} disconnect() {} };
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  delete window.__buildTerminal;
  delete window.ResizeObserver;
});

describe("terminal pane resource ownership", () => {
  const options = (attach) => ({ attach, input: vi.fn(async () => {}), resize: vi.fn(async () => {}), isTouchDevice: () => true });

  it.each(["scope", "signal"])("does not acquire resources after its delayed load loses %s ownership", async (ownership) => {
    const host = document.createElement("div");
    host.innerHTML = '<span class="replacement">Replacement pane</span>';
    const replacement = host.firstElementChild;
    const config = options(vi.fn(async () => {}));
    const controller = new AbortController();
    const mounted = mountTerminalPane(host, { ...config, isCurrent: () => ownership !== "scope", signal: controller.signal });
    controller.abort();
    await expect(mounted).rejects.toThrow("closed");
    expect(terminals).toHaveLength(0);
    expect(host.firstElementChild).toBe(replacement);
    expect(config.attach).not.toHaveBeenCalled();
  });

  it("cleans every acquired resource when attach rejects", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const viewport = new EventTarget();
    const removeViewportListener = vi.spyOn(viewport, "removeEventListener");
    vi.stubGlobal("visualViewport", viewport);
    let screen, bar;
    const config = options(async () => {
      screen = host.querySelector(".term-screen");
      bar = host.querySelector(".termkeys");
      throw new Error("attach rejected");
    });
    const removeListener = vi.spyOn(EventTarget.prototype, "removeEventListener");
    await expect(mountTerminalPane(host, config)).rejects.toThrow("attach rejected");
    const term = terminals[0];
    expect(term.dispose).toHaveBeenCalledOnce();
    expect(term.dataSubscription.dispose).toHaveBeenCalledOnce();
    expect(term.resizeSubscription.dispose).toHaveBeenCalledOnce();
    expect(host.children).toHaveLength(0);
    expect(window.__buildTerminal).toBeUndefined();
    for (const name of ["touchstart", "touchmove", "touchend", "touchcancel", "focusin"]) {
      expect(removeListener.mock.calls.some((args, i) => removeListener.mock.contexts[i] === screen && args[0] === name)).toBe(true);
    }
    press(bar.querySelector('[data-key="esc"]'));
    screen.dispatchEvent(new Event("focusin"));
    expect(config.input).not.toHaveBeenCalled();
    expect(window.__buildTerminal).toBeUndefined();
    expect(removeViewportListener.mock.calls.map(([name]) => name)).toEqual(["resize", "scroll"]);
  });

  it("cleans partial setup when fitting throws before attach", async () => {
    setup.fitError = new Error("fit rejected");
    const host = document.createElement("div");
    const config = options(vi.fn(async () => {}));
    const removeListener = vi.spyOn(EventTarget.prototype, "removeEventListener");
    await expect(mountTerminalPane(host, config)).rejects.toThrow("fit rejected");
    expect(terminals[0].dispose).toHaveBeenCalledOnce();
    expect(host.children).toHaveLength(0);
    expect(config.attach).not.toHaveBeenCalled();
    for (const name of ["touchstart", "touchmove", "touchend", "touchcancel"]) {
      expect(removeListener.mock.calls.some(([type]) => type === name)).toBe(true);
    }
  });

  it("stops touch momentum if attachment fails while a flick is coasting", async () => {
    const requestFrame = vi.spyOn(window, "requestAnimationFrame").mockReturnValue(73);
    const cancelFrame = vi.spyOn(window, "cancelAnimationFrame");
    const host = document.createElement("div");
    const touch = (screen, type, clientY, timeStamp) => {
      const point = { identifier: 1, clientY };
      const event = new Event(type, { cancelable: true });
      Object.defineProperties(event, {
        touches: { value: type === "touchend" ? [] : [point] },
        changedTouches: { value: [point] }, timeStamp: { value: timeStamp },
      });
      screen.dispatchEvent(event);
    };
    await expect(mountTerminalPane(host, options(async () => {
      const screen = host.querySelector(".term-screen");
      touch(screen, "touchstart", 100, 1);
      touch(screen, "touchmove", 20, 40);
      touch(screen, "touchend", 0, 50);
      throw new Error("attach rejected");
    }))).rejects.toThrow("attach rejected");
    expect(requestFrame).toHaveBeenCalledOnce();
    expect(cancelFrame).toHaveBeenCalledExactlyOnceWith(73);
  });

  it("disposes once and leaves a replacement's DOM and QA terminal standing", async () => {
    const { host, pane, term } = await mount();
    const disconnect = vi.fn();
    window.ResizeObserver = class { observe() {} disconnect = disconnect; };
    const replacement = await mountTerminalPane(host, options(async () => {}));
    const screen = host.querySelector(".term-screen");
    const bar = host.querySelector(".termkeys");
    pane.dispose(); pane.dispose();
    expect(term.dispose).toHaveBeenCalledOnce();
    expect(host.querySelector(".term-screen")).toBe(screen);
    expect(host.querySelector(".termkeys")).toBe(bar);
    expect(window.__buildTerminal).toBe(replacement.terminal);
    replacement.dispose(); replacement.dispose();
    expect(disconnect).toHaveBeenCalledOnce();
    expect(window.__buildTerminal).toBeUndefined();
  });

  it("keeps a newer pane when an older attach rejects late", async () => {
    const host = document.createElement("div");
    let reject;
    const old = mountTerminalPane(host, options(() => new Promise((_, fail) => { reject = fail; })));
    const outcome = old.catch((error) => error.message);
    await vi.waitFor(() => expect(reject).toBeTypeOf("function"));
    const replacement = await mountTerminalPane(host, options(async () => {}));
    const screen = host.querySelector(".term-screen");
    reject(new Error("old attach rejected"));
    expect(await outcome).toBe("old attach rejected");
    expect(terminals[0].dispose).toHaveBeenCalledOnce();
    expect(host.querySelector(".term-screen")).toBe(screen);
    expect(window.__buildTerminal).toBe(replacement.terminal);
    replacement.dispose();
  });
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
