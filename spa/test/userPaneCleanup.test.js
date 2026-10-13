// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const harness = vi.hoisted(() => ({ registrations: new Map(), terminals: [], replaceDuringObserve: false }));
const manager = vi.hoisted(() => ({
  detachOwned: vi.fn((id, entry) => {
    if (harness.registrations.get(id) === entry) harness.registrations.delete(id);
  }),
  attachTerminal: vi.fn(async (id, scope, options) => {
    const entry = { scope, options };
    harness.registrations.set(id, entry);
    options.onRegistered(() => manager.detachOwned(id, entry));
    return { snapshot: "", cursor: 0 };
  }),
  input: vi.fn(async () => {}), resize: vi.fn(async () => {}),
}));
vi.mock("../src/terminal/manager.js", () => ({
  terminalManager: () => manager, terminalDeviceId: () => "device",
  subscribeTerminalStatus: () => () => {}, followTerminalDevice: () => {},
  resetTerminalManager: () => {}, provideTerminalSessions: () => {},
}));
vi.mock("ghostty-web", () => ({
  init: async () => {},
  Terminal: class {
    constructor() {
      this.cols = 80; this.rows = 24;
      this.buffer = { active: { type: "normal" } };
      this.dispose = vi.fn();
      this.dataSubscription = { dispose: vi.fn() };
      this.resizeSubscription = { dispose: vi.fn() };
      harness.terminals.push(this);
    }
    open() {}
    loadAddon() {}
    onData() { return this.dataSubscription; }
    onResize() { return this.resizeSubscription; }
  },
  FitAddon: class { fit() {} proposeDimensions() { return { cols: 80, rows: 24 }; } },
}));

const { mountUserTerminalPane } = await import("../src/core/console.js");
const replacement = { replacement: true };

beforeEach(() => {
  harness.registrations.clear(); harness.terminals.length = 0;
  manager.attachTerminal.mockClear(); manager.detachOwned.mockClear();
  document.body.innerHTML = '<div id="pane"></div>';
  vi.stubGlobal("ResizeObserver", class {
    observe() {
      if (harness.replaceDuringObserve) harness.registrations.set("shell", replacement);
      throw new Error("observer setup failed");
    }
    disconnect() {}
  });
});
afterEach(() => { vi.unstubAllGlobals(); delete window.__buildTerminal; });

describe("user terminal pane setup failure after successful attach", () => {
  it.each([false, true])("detaches its owned registration and preserves replacement=%s", async (replaceDuringObserve) => {
    harness.replaceDuringObserve = replaceDuringObserve;
    const host = document.getElementById("pane");
    await expect(mountUserTerminalPane(host, "shell", {
      scope: { project_id: "project", source_id: "source", source_path: "/source" }, deviceId: "device",
    })).rejects.toThrow("observer setup failed");
    expect(manager.attachTerminal).toHaveBeenCalledOnce();
    expect(manager.detachOwned).toHaveBeenCalledOnce();
    expect(harness.registrations.get("shell")).toBe(replaceDuringObserve ? replacement : undefined);
    const term = harness.terminals[0];
    expect(term.dispose).toHaveBeenCalledOnce();
    expect(term.dataSubscription.dispose).toHaveBeenCalledOnce();
    expect(term.resizeSubscription.dispose).toHaveBeenCalledOnce();
    expect(host.children).toHaveLength(0);
    expect(window.__buildTerminal).toBeUndefined();
  });
});
