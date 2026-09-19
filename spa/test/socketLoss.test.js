// @vitest-environment jsdom
// What a surface does when the terminal socket is not there.
//
// A lost socket is not "this checkout has no terminals" and not "no agent has
// ever run here" — it is the machine being out of reach for a moment. The
// surfaces that ask the socket for something say so, and ask again the moment it
// is back, on the socket's OWN reconnect (never a timer of their own, which is
// how retries used to stack up behind a dead socket).

import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";

/** The one bridge this file's device answers through: a test that hands over
 *  a new `call` is that bridge answering differently, not another machine. */
const bridge = { call: null };

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
  // Which machine the shells type at, and the moves between machines, are the
  // app spine's business and not this suite's: they answer, and nothing moves.
  terminalDeviceId: () => null,
  followTerminalDevice: () => {},
  resetTerminalManager: () => {},
  // Minting a terminal session is the connection layer's (spec rule 5); no
  // suite here opens one.
  provideTerminalSessions: () => {},
}));

// No ghostty/wasm under node: the pane is a leaf that performs the attach it was
// handed, so an attach that rejects rejects the mount.
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { dispose: vi.fn() };
  },
}));

const { consoleCacheScope, emptyConsoleWorld, seedConsoleWorld } = await import("./consoleWorld.js");
const { TerminalSocketLost } = await import("../src/terminal/session.js");
const { App } = await import("../src/app.js");
const { mountConsole, resetConsoleMemory } = await import("../src/core/console.js");
const { mountAgentTab } = await import("../src/core/surfaceTabs.js");

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};
const lost = () => new TerminalSocketLost("disconnected");
const host = () => document.body.appendChild(document.createElement("div"));

// A console left standing hears the next case's cache writes and mounts its
// pane again over them: every one this suite opens is torn down after it.
const mounted = [];

beforeEach(async () => {
  document.body.innerHTML = "";
  localStorage.clear();
  resetConsoleMemory();
  await emptyConsoleWorld();
  status.reset();
  for (const fn of Object.values(manager)) fn.mockReset();
  manager.input.mockResolvedValue(undefined);
  manager.resize.mockResolvedValue(undefined);
  bridge.call = vi.fn(async () => ({ project_id: "p1", branch: "build/login", run_id: "run-3" }));
});

afterEach(() => {
  while (mounted.length) mounted.pop().dispose();
});

const branch = {
  kind: "branch",
  deviceId: "dev-1",
  projectId: "p1",
  branch: "build/login",
  call: (...args) => bridge.call(...args),
  cacheScope: consoleCacheScope(),
};

/** An open console on a branch whose checkout resolves, over the terminals the
 *  cache says that checkout is holding. */
async function openConsole(terminals = ["term-1"]) {
  await seedConsoleWorld({ terminals });
  const region = host();
  const panel = mountConsole(region, branch);
  mounted.push(panel);
  await flush();
  panel.toggle(); // the console opens at half, and lists what is there
  await flush();
  return {
    region,
    panel,
    // What the console says INSTEAD of a screen — the failed-mount line, or the
    // line for a checkout with nowhere to open a shell. A mounted pane says
    // nothing (its connectivity chip is hidden until the status hub says so).
    said: () => region.querySelector(".console-body .console-empty, .console-body .empty")?.textContent ?? "",
  };
}

// The tab strip comes off the cache, so a lost socket takes nothing off it:
// the console still says which shells that checkout is holding. What the
// socket carries is the SCREEN, and that is what this is about.
describe("the console when the socket is lost", () => {
  it("says the machine is out of reach rather than 'no terminals', and asks again when it is back", async () => {
    // Nothing has ever said what this checkout is holding, so the one list is
    // the console's to make — and it is the list that the lost socket cuts off.
    manager.listTerminals.mockRejectedValue(lost());
    const { region, said } = await openConsole(null);
    expect(said()).toMatch(/reconnecting/i);
    // The offer to open a shell is withheld: a `+` here would put a second one
    // beside whatever is already running in that checkout.
    expect(region.querySelector(".console-new")).toBeNull();

    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    status.set("connected");
    await flush();
    expect(manager.listTerminals).toHaveBeenCalledTimes(2);
    expect([...region.querySelectorAll(".console-tab-name")].map((c) => c.textContent)).toEqual(["Terminal 1"]);
    expect(region.querySelector(".console-new")).not.toBeNull();
  });

  it("re-mounts a pane whose attach was cut off, rather than calling the terminal unavailable", async () => {
    manager.attachTerminal.mockRejectedValueOnce(lost()).mockResolvedValue({ snapshot: "", cursor: 0 });
    const { region, said } = await openConsole();
    expect(said()).toMatch(/reconnecting/i);
    expect(said()).not.toMatch(/unavailable/i);
    // The tabs are the cache's, so they are all still there to come back to.
    expect([...region.querySelectorAll(".console-tab-name")].map((c) => c.textContent)).toEqual(["Terminal 1"]);

    status.set("connected");
    await flush();
    expect(manager.attachTerminal).toHaveBeenCalledTimes(2);
    expect(said()).toBe(""); // the screen is back
    expect(region.querySelector(".console-pane")).not.toBeNull();
  });

  it("still reports a real failure as one", async () => {
    manager.attachTerminal.mockRejectedValue(new Error("pty is gone"));
    const { said } = await openConsole();
    expect(said()).toMatch(/pty is gone/);
  });

  it("leaves no watcher behind when the console is torn down while it waits", async () => {
    manager.attachTerminal.mockRejectedValue(lost());
    const { panel } = await openConsole();
    expect(status.watchers()).toBe(1);
    panel.dispose();
    status.set("connected");
    await flush();
    expect(status.watchers()).toBe(0);
    expect(manager.attachTerminal).toHaveBeenCalledTimes(1);
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
