// @vitest-environment jsdom
// What a surface does when the terminal socket is not there.
//
// A lost socket is not "this checkout has no terminals" and not "no agent has
// ever run here" — it is the machine being out of reach for a moment. The
// surfaces that ask the socket for something say so, and ask again the moment it
// is back, on the socket's OWN reconnect (never a timer of their own, which is
// how retries used to stack up behind a dead socket).

import { describe, it, expect, vi, beforeEach } from "vitest";

// The shared socket's status fan-out, as a double a test can drive: the real one
// is what tells a surface its machine is reachable again.
const status = vi.hoisted(() => {
  const subscribers = new Set();
  let current = null;
  return {
    set(value) {
      current = value;
      for (const subscriber of [...subscribers]) subscriber(value);
    },
    subscribe(fn) {
      subscribers.add(fn);
      if (current !== null) fn(current);
      return () => subscribers.delete(fn);
    },
    watchers: () => subscribers.size,
    reset() {
      subscribers.clear();
      current = null;
    },
  };
});

const manager = vi.hoisted(() => ({
  listTerminals: vi.fn(),
  createTerminal: vi.fn(),
  closeTerminal: vi.fn(),
  attachTerminal: vi.fn(),
  attachAgent: vi.fn(),
  input: vi.fn(async () => {}),
  resize: vi.fn(async () => {}),
  detach: vi.fn(),
}));

vi.mock("../src/terminal/manager.js", () => ({
  terminalManager: () => manager,
  subscribeTerminalStatus: status.subscribe,
}));

// No ghostty/wasm under node: the pane is a leaf that performs the attach it was
// handed, so an attach that rejects rejects the mount.
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { dispose: vi.fn() };
  },
}));

const { TerminalSocketLost } = await import("../src/terminal/session.js");
const { App } = await import("../src/app.js");
const { mountConsole, resetConsoleMemory } = await import("../src/core/console.js");
const { mountAgentTab } = await import("../src/core/surfaceTabs.js");

const flush = async () => {
  for (let i = 0; i < 6; i++) await new Promise((done) => setTimeout(done, 0));
};
const lost = () => new TerminalSocketLost("disconnected");
const host = () => document.body.appendChild(document.createElement("div"));

beforeEach(() => {
  document.body.innerHTML = "";
  localStorage.clear();
  resetConsoleMemory();
  status.reset();
  for (const fn of Object.values(manager)) fn.mockReset();
  manager.input.mockResolvedValue(undefined);
  manager.resize.mockResolvedValue(undefined);
  App.call = vi.fn(async () => ({ project_id: "p1", branch: "build/login", run_id: "run-3" }));
});

const branch = { kind: "branch", projectId: "p1", branch: "build/login" };

/** An open console on a branch whose checkout resolves. */
async function openConsole() {
  const region = host();
  const panel = mountConsole(region, branch);
  await flush();
  panel.toggle(); // the console opens at half, and lists what is there
  await flush();
  return {
    region,
    panel,
    // What the console says INSTEAD of a screen — the failed-listing line or the
    // failed-mount one. A mounted pane says nothing (its connectivity chip is
    // hidden until the status hub says otherwise).
    said: () => region.querySelector(".console-body .console-empty, .console-body .empty")?.textContent ?? "",
  };
}

describe("the console when the socket is lost", () => {
  it("says the machine is out of reach instead of 'no terminals', and lists again when it is back", async () => {
    manager.listTerminals.mockRejectedValue(lost());
    const { region, said } = await openConsole();
    expect(said()).toMatch(/reconnecting/i);
    // The offer to open a shell is withheld: creating one needs the socket too.
    expect(region.querySelector(".console-start")).toBeNull();

    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    status.set("connected");
    await flush();
    expect(manager.listTerminals).toHaveBeenCalledTimes(2);
    expect([...region.querySelectorAll(".console-tab-name")].map((c) => c.textContent)).toEqual(["Terminal 1"]);
  });

  it("re-mounts a pane whose attach was cut off, rather than calling the terminal unavailable", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    manager.attachTerminal.mockRejectedValueOnce(lost()).mockResolvedValue({ snapshot: "", cursor: 0 });
    const { region, said } = await openConsole();
    expect(said()).toMatch(/reconnecting/i);
    expect(said()).not.toMatch(/unavailable/i);

    status.set("connected");
    await flush();
    expect(manager.attachTerminal).toHaveBeenCalledTimes(2);
    expect(said()).toBe(""); // the screen is back
    expect(region.querySelector(".console-pane")).not.toBeNull();
  });

  it("still reports a real failure as one", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    manager.attachTerminal.mockRejectedValue(new Error("pty is gone"));
    const { said } = await openConsole();
    expect(said()).toMatch(/pty is gone/);
  });

  it("leaves no watcher behind when the console is torn down while it waits", async () => {
    manager.listTerminals.mockRejectedValue(lost());
    const { panel } = await openConsole();
    expect(status.watchers()).toBe(1);
    panel.dispose();
    status.set("connected");
    await flush();
    expect(status.watchers()).toBe(0);
    expect(manager.listTerminals).toHaveBeenCalledTimes(1);
  });
});

describe("the agent tab when the socket is lost", () => {
  it("says the machine is out of reach, and attaches again when it is back", async () => {
    manager.attachAgent.mockRejectedValueOnce(lost()).mockImplementation(async (target, opts) => {
      const result = { term_id: "agent:wt-3", snapshot: "", cursor: 0, live: true };
      opts.onLive(true, result);
      return result;
    });
    const el = host();
    const tab = mountAgentTab(el, { id: "run-3" });
    await flush();
    expect(el.querySelector("#agentOverlay").hidden).toBe(false);
    expect(el.querySelector("#agentOverlayMsg").textContent).toMatch(/reconnecting/i);

    status.set("connected");
    await flush();
    expect(manager.attachAgent).toHaveBeenCalledTimes(2);
    expect(el.querySelector("#agentOverlay").hidden).toBe(true);
    tab.dispose();
    expect(status.watchers()).toBe(0);
  });

  it("still shows the idle offer when the attach fails for its own reasons", async () => {
    manager.attachAgent.mockRejectedValue(new Error("unknown id"));
    const el = host();
    mountAgentTab(el, { id: "run-3" }, { idleLabel: "No agent is currently running" });
    await flush();
    expect(el.querySelector("#agentOverlayMsg").textContent).toBe("No agent is currently running");
  });
});
