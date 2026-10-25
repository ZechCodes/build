// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const harness = vi.hoisted(() => ({ registered: new Map(), starts: [], status: null, statusListeners: new Set(), subscriptions: 0, reads: [], blockRead: null }));
const manager = vi.hoisted(() => ({
  listTerminals: vi.fn(async () => []),
  createTerminal: vi.fn(async () => ({ term_id: "new-term" })),
  closeTerminal: vi.fn(async () => {}),
  attachTerminal: vi.fn(async (id, scope) => {
    harness.registered.set(id, scope);
    return { snapshot: "", cursor: 0 };
  }),
  detach: vi.fn((id) => harness.registered.delete(id)),
}));
vi.mock("../src/core/localCache.js", async (original) => {
  const cache = await original();
  return { ...cache, readCached: async (address) => {
    harness.reads.push(address);
    const record = await cache.readCached(address);
    await harness.blockRead?.(address);
    return record;
  } };
});
vi.mock("../src/terminal/manager.js", () => ({
  terminalManager: () => manager,
  terminalDeviceId: () => "dev-1",
  subscribeTerminalStatus: (listener) => {
    harness.subscriptions += 1;
    harness.statusListeners.add(listener);
    if (harness.status) listener(harness.status);
    return () => harness.statusListeners.delete(listener);
  },
  followTerminalDevice: () => {}, resetTerminalManager: () => {}, provideTerminalSessions: () => {},
}));
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, options) => {
    // The real pane loads WASM before attach: model that uncancellable delay.
    await new Promise((resolve) => harness.starts.push(resolve));
    await options.attach({ cols: 80, rows: 24 });
    return { dispose() { host.innerHTML = ""; } };
  },
}));

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { mountProjectConsole } = await import("../src/core/projectConsole.js");
const { deleteCached, readCached, readCanonicalCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { wipeUiRecords } = await import("../src/core/localUiStore.js");
const { TerminalSocket } = await import("../src/terminal/session.js");
const flush = async () => { for (let step = 0; step < 15; step++) await new Promise((done) => setTimeout(done, 0)); };
const projectsAddress = { deviceId: "dev-1", entityId: "", kind: "projects" };
const rosterAddress = { deviceId: "dev-1", entityId: 'project:["p1","code","/code"]', kind: "terminals" };
const movedAddress = { deviceId: "dev-1", entityId: 'project:["p1","code","/moved"]', kind: "terminals" };
const moveSource = () => writeCached(projectsAddress, [{ project_id: "p1", sources: [{ id: "code", name: "Code", path: "/moved" }] }]);
let panel;

beforeEach(async () => {
  await wipeCache(); await wipeUiRecords();
  harness.registered.clear(); harness.starts.length = 0;
  harness.reads = []; harness.blockRead = null;
  harness.status = null; harness.statusListeners.clear(); harness.subscriptions = 0;
  manager.detach.mockClear();
  manager.listTerminals.mockReset().mockResolvedValue([]);
  manager.createTerminal.mockReset().mockResolvedValue({ term_id: "new-term" });
  manager.closeTerminal.mockReset().mockResolvedValue(undefined);
  document.body.innerHTML = '<div id="console-region"></div>';
  await writeCached({ deviceId: "dev-1", entityId: "", kind: "projects" }, [{ project_id: "p1", sources: [{ id: "code", name: "Code", path: "/code" }] }]);
  await writeCached({ deviceId: "dev-1", entityId: "", kind: "project-terminal-support" }, { projectSources: true });
  await writeCached({ deviceId: "dev-1", entityId: 'project:["p1","code","/code"]', kind: "terminals" }, { tabs: [{ term_id: "term-1" }] });
  panel = mountProjectConsole(document.getElementById("console-region"), { kind: "project", deviceId: "dev-1", projectId: "p1" });
  await flush();
});

describe("project console source picker", () => {
  const picker = () => document.querySelector("[aria-label='Terminal source']");

  it("shows a sole cached source and its folder icon while keeping it selected for new shells", async () => {
    expect(getComputedStyle(picker()).visibility).toBe("visible");
    expect(getComputedStyle(document.querySelector(".console-source-icon")).visibility).toBe("visible");
    picker().focus();
    expect(document.activeElement).toBe(picker());
    expect(picker().value).toBe("code");
    document.querySelector(".console-new").click(); await flush();
    expect(manager.createTerminal).toHaveBeenCalledWith({ project_id: "p1", source_id: "code", source_path: "/code" }, 80, 24);
  });

  it("also shows the picker for a legacy project's single path", async () => {
    await writeCached(projectsAddress, [{ project_id: "p1", name: "Code", path: "/code" }]);
    await flush();
    expect(getComputedStyle(picker()).visibility).toBe("visible");
    expect(getComputedStyle(document.querySelector(".console-source-icon")).visibility).toBe("visible");
    picker().focus();
    expect(document.activeElement).toBe(picker());
    expect(picker().value).toBe("primary");
  });

  it("keeps an empty source picker disabled", async () => {
    await writeCached(projectsAddress, [{ project_id: "p1", sources: [] }]);
    await flush();
    expect(picker().disabled).toBe(true);
    expect(document.querySelector(".console-new").disabled).toBe(true);
  });
});

describe("prompt project console actions", () => {
  it("ignores continuous unrelated thread pushes on the same device", async () => {
    harness.reads = [];
    for (let index = 0; index < 20; index++) {
      await writeCached({ deviceId: "dev-1", entityId: "thread-1", kind: "feed" }, { index });
      await new Promise((done) => setTimeout(done, 0));
    }
    await flush();
    expect(harness.reads).toEqual([]);
  });

  it("creates and closes immediately while unrelated thread pushes continue", async () => {
    let pushing = true;
    let pushes = 0;
    const flood = (async () => {
      while (pushing) {
        await writeCached({ deviceId: "dev-1", entityId: "thread-1", kind: "feed" }, { sequence: ++pushes });
        await new Promise((done) => setTimeout(done, 0));
      }
    })();
    try {
      document.querySelector(".console-new").click();
      expect(manager.createTerminal).toHaveBeenCalledOnce();
      await flush();
      expect(panel.size()).toBe("half");
      const createdTab = [...document.querySelectorAll(".console-tab")].find((tab) => tab.dataset.key.includes("new-term"));
      expect(createdTab).toBeDefined();
      createdTab.querySelector("[data-close]").click();
      expect(manager.closeTerminal).toHaveBeenCalledWith("new-term", { project_id: "p1", source_id: "code", source_path: "/code" });
      await flush();
      expect((await readCached(rosterAddress)).value.tabs).toEqual([{ term_id: "term-1" }]);
      expect(pushes).toBeGreaterThan(1);
    } finally { pushing = false; await flood; }
  });

  it("opens a newly created shell without waiting for an in-flight background cache read", async () => {
    let release;
    const blocked = new Promise((resolve) => { release = resolve; });
    harness.blockRead = (address) => {
      if (address.kind === "terminals") { harness.blockRead = null; return blocked; }
    };
    try {
      await writeCached({ deviceId: "dev-1", entityId: "", kind: "project-terminal-support" }, { projectSources: true });
      await flush();
      document.querySelector(".console-new").click();
      expect(manager.createTerminal).toHaveBeenCalledOnce();
      await flush();
      expect((await readCanonicalCached(rosterAddress)).value.tabs).toContainEqual({ term_id: "new-term" });
      expect(panel.size()).toBe("half");
      const createdTab = [...document.querySelectorAll(".console-tab")].find((tab) => tab.dataset.key.includes("new-term"));
      expect(createdTab).toBeDefined();
      createdTab.querySelector("[data-close]").click();
      expect(manager.closeTerminal).toHaveBeenCalledWith("new-term", { project_id: "p1", source_id: "code", source_path: "/code" });
      await flush();
      expect([...document.querySelectorAll(".console-tab")].some((tab) => tab.dataset.key.includes("new-term"))).toBe(false);
    } finally {
      harness.blockRead = null; release(); await flush();
    }
  });

  it("does not restore a closed tab when an older cached roster read finishes", async () => {
    let release;
    const blocked = new Promise((resolve) => { release = resolve; });
    harness.blockRead = (address) => {
      if (address.kind === "terminals") { harness.blockRead = null; return blocked; }
    };
    try {
      await writeCached({ deviceId: "dev-1", entityId: "", kind: "project-terminal-support" }, { projectSources: true });
      await flush();
      document.querySelector("[data-close]").click(); await flush();
      expect(document.querySelectorAll(".console-tab")).toHaveLength(0);
    } finally {
      harness.blockRead = null; release(); await flush();
    }
    expect(document.querySelectorAll(".console-tab")).toHaveLength(0);
  });

  it.each(["removal", "replacement"])("does not publish create completion after an unseen source %s", async (change) => {
    let finish;
    manager.createTerminal.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    document.querySelector(".console-new").click();
    let release;
    const blocked = new Promise((resolve) => { release = resolve; });
    harness.blockRead = () => blocked;
    try {
      if (change === "replacement") await moveSource();
      else await writeCached(projectsAddress, [{ project_id: "p1", sources: [] }]);
      finish({ term_id: "late-term" }); await flush();
      expect(panel.size()).toBe("collapsed");
    } finally {
      harness.blockRead = null; release(); await flush();
    }
    expect(panel.size()).toBe("collapsed");
    expect((await readCached(rosterAddress)).value.tabs).toEqual([{ term_id: "term-1" }]);
    expect(document.querySelectorAll(".console-tab")).toHaveLength(0);
  });
});

describe("late project console roster responses", () => {
  it("re-reads a replacement source invalidated during its first roster collection", async () => {
    await writeCached(movedAddress, { tabs: [] });
    let release;
    const blocked = new Promise((resolve) => { release = resolve; });
    let captured = false;
    harness.blockRead = (address) => {
      if (address.entityId === movedAddress.entityId && address.kind === "terminals") {
        captured = true; harness.blockRead = null; return blocked;
      }
    };
    try {
      await moveSource(); await flush();
      expect(captured).toBe(true);
      expect(document.querySelector(".console-tab").dataset.key).toContain("/code");
      harness.reads = [];
      await writeCached({ deviceId: "dev-1", entityId: 'project:["other","code","/moved"]', kind: "terminals" }, { tabs: [] });
      await writeCached({ deviceId: "dev-1", entityId: "thread-1", kind: "feed" }, { sequence: 1 });
      await flush();
      expect(harness.reads).toEqual([]);
      await writeCached(movedAddress, { tabs: [{ term_id: "replacement-term", name: "Pushed" }] });
      release(); await flush();
      expect(document.querySelector(".console-tab-name")?.textContent).toBe("Code · Pushed");
      expect(document.querySelector(".console-tab").dataset.key).toContain("/moved");
    } finally {
      harness.blockRead = null; release(); await flush();
    }
  });

  it("retries a failed missing roster on a later relevant cache update", async () => {
    manager.listTerminals.mockRejectedValueOnce(new Error("socket lost"))
      .mockResolvedValueOnce([{ term_id: "retried-term" }]);
    await deleteCached([rosterAddress]); await flush();
    expect(manager.listTerminals).toHaveBeenCalledTimes(1);
    expect(await readCached(rosterAddress)).toBeUndefined();
    await writeCached({ deviceId: "dev-1", entityId: "", kind: "project-terminal-support" }, { projectSources: true });
    await flush();
    expect(manager.listTerminals).toHaveBeenCalledTimes(2);
    expect((await readCached(rosterAddress)).value.tabs).toEqual([{ term_id: "retried-term" }]);
  });

  it("does not cache a source list response after the source moved", async () => {
    let finish;
    manager.listTerminals.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    await deleteCached([rosterAddress]); await flush();
    expect(manager.listTerminals).toHaveBeenCalledOnce();
    await moveSource(); await flush();
    finish([{ term_id: "late-term" }]); await flush();
    expect(await readCached(rosterAddress)).toBeUndefined();
    expect((await readCached(movedAddress)).value.tabs).toEqual([]);
  });

  it("does not add a newly created shell to a replacement source", async () => {
    let finish;
    manager.createTerminal.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    document.querySelector(".console-new").click(); await flush();
    await moveSource(); await flush();
    finish({ term_id: "late-term" }); await flush();
    expect((await readCached(rosterAddress)).value.tabs).toEqual([{ term_id: "term-1" }]);
    expect((await readCached(movedAddress)).value.tabs).toEqual([]);
    expect(panel.size()).toBe("collapsed");
    expect(document.querySelectorAll(".console-tab")).toHaveLength(0);
    await flush();
    expect(document.querySelectorAll(".console-tab")).toHaveLength(0);
  });

  it("does not update a disposed console when a shell creation completes", async () => {
    let finish;
    manager.createTerminal.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    document.querySelector(".console-new").click(); await flush();
    panel.dispose(); panel = null;
    finish({ term_id: "late-term" }); await flush();
    expect((await readCached(rosterAddress)).value.tabs).toEqual([{ term_id: "term-1" }]);
    expect(document.getElementById("console-region").innerHTML).toBe("");
  });

  it("does not redraw an old-path tab after replacement during close and a late roster read", async () => {
    let finish;
    manager.closeTerminal.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    document.querySelector("[data-close]").click();
    let release;
    const blocked = new Promise((resolve) => { release = resolve; });
    harness.blockRead = (address) => {
      if (address.entityId === rosterAddress.entityId && address.kind === "terminals") {
        harness.blockRead = null; return blocked;
      }
    };
    let observer;
    const staleTabs = [];
    try {
      await writeCached({ deviceId: "dev-1", entityId: "", kind: "project-terminal-support" }, { projectSources: true });
      await flush();
      await writeCached(movedAddress, { tabs: [{ term_id: "term-1", name: "Replacement" }] });
      await moveSource();
      finish(); await flush();
      const tabs = () => [...document.querySelectorAll(".console-tab")];
      expect(tabs()).toHaveLength(1);
      expect(tabs()[0].dataset.key).toContain("/moved");
      expect(document.querySelector(".console-tab-name").textContent).toBe("Code · Replacement");
      observer = new MutationObserver((changes) => {
        for (const change of changes) for (const node of change.addedNodes) {
          if (node.nodeType !== Node.ELEMENT_NODE) continue;
          for (const tab of [node, ...node.querySelectorAll(".console-tab")]) {
            if (tab.matches(".console-tab") && tab.dataset.key.includes("/code")) staleTabs.push(tab.dataset.key);
          }
        }
      });
      observer.observe(document.getElementById("console-region"), { childList: true, subtree: true });
      release(); await flush();
      expect(staleTabs).toEqual([]);
      expect(tabs()).toHaveLength(1);
      expect(tabs()[0].dataset.key).toContain("/moved");
      expect((await readCached(rosterAddress)).value.tabs).toEqual([{ term_id: "term-1" }]);
      expect((await readCached(movedAddress)).value.tabs).toEqual([{ term_id: "term-1", name: "Replacement" }]);
    } finally {
      observer?.disconnect(); harness.blockRead = null; release(); await flush();
    }
  });

  it("does not remove a replacement source's same-ID shell when the old close settles", async () => {
    let finish;
    manager.closeTerminal.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    document.querySelector("[data-close]").click(); await flush();
    await writeCached(movedAddress, { tabs: [{ term_id: "term-1", name: "Replacement" }] });
    await moveSource(); await flush();
    finish(); await flush();
    expect((await readCached(movedAddress)).value.tabs).toEqual([{ term_id: "term-1", name: "Replacement" }]);
    expect(document.querySelector(".console-tab-name").textContent).toBe("Code · Replacement");
  });
});

describe("project terminal device authority", () => {
  it("does not send a source creation queued on one device to the next device", async () => {
    const sent = vi.fn(async () => ({ term_id: "term-1" }));
    const socket = new TerminalSocket({ transport: {}, userTerminalRpc: sent });
    socket._session = { deviceId: "dev-a" };
    const creating = socket.createTerminal({ project_id: "p1", source_id: "code", source_path: "/code" }, 80, 24);
    const outcome = creating.then(() => "sent", () => "refused");
    socket._session = { deviceId: "dev-b" };
    socket._connected = true;
    for (const waiter of socket._connectWaiters.splice(0)) waiter.resolve();
    expect(await outcome).toBe("refused");
    expect(sent).not.toHaveBeenCalled();
  });

  it("does not forget a replacement registration when an earlier tab close settles", async () => {
    let finish;
    const socket = new TerminalSocket({ transport: {}, userTerminalRpc: () => new Promise((resolve) => { finish = resolve; }) });
    socket._session = { deviceId: "dev-a" };
    socket._connected = true;
    const scope = { project_id: "p1", source_id: "code", source_path: "/code" };
    socket._register("term-1", "user", scope, {});
    const closing = socket.closeTerminal("term-1", scope);
    await Promise.resolve();
    socket._session = { deviceId: "dev-b" };
    const replacement = socket._register("term-1", "user", scope, {});
    finish({}); await closing;
    expect(socket._terms.get("term-1")).toBe(replacement);
  });
});
afterEach(() => { panel?.dispose(); panel = null; });

describe("project console pane authority", () => {
  it("keeps its pending pane across connection transitions with unchanged cache", async () => {
    panel.toggle(); await flush();
    const pane = document.querySelector(".console-pane");
    const before = document.getElementById("console-region").innerHTML;
    expect(harness.starts).toHaveLength(1);
    for (const status of ["connected", "disconnected", "connecting", "connected"]) {
      harness.status = status;
      for (const listener of harness.statusListeners) listener(status);
      await flush();
      expect(document.querySelector(".console-pane")).toBe(pane);
      expect(document.getElementById("console-region").innerHTML).toBe(before);
      expect(harness.starts).toHaveLength(1);
    }
    expect(harness.subscriptions).toBe(0);
  });

  it("old console disposal cannot detach a replacement same-ID registration", async () => {
    const socket = new TerminalSocket({ transport: {}, userTerminalRpc: async () => ({ snapshot: "", cursor: 0 }) });
    socket._session = { deviceId: "dev-1" };
    socket._connected = true;
    const originalAttach = manager.attachTerminal.getMockImplementation();
    const originalDetach = manager.detach.getMockImplementation();
    manager.attachTerminal.mockImplementation((...args) => socket.attachTerminal(...args));
    manager.detach.mockImplementation((...args) => socket.detach(...args));
    try {
      panel.toggle(); await flush();
      harness.starts[0](); await flush();
      expect(socket._terms.size).toBe(1);
      const replacement = socket._register("term-1", "user", { project_id: "other" }, {});
      panel.dispose(); panel = null;
      await flush();
      expect(socket._terms.get("term-1")).toBe(replacement);
    } finally {
      socket.close();
      manager.attachTerminal.mockImplementation(originalAttach);
      manager.detach.mockImplementation(originalDetach);
    }
  });
  it("cancels the real socket attach waiting for connection when the console collapses", async () => {
    const dispatch = vi.fn(async () => ({ snapshot: "", cursor: 0 }));
    const socket = new TerminalSocket({ transport: {}, userTerminalRpc: dispatch });
    socket._session = { deviceId: "dev-1" };
    const originalAttach = manager.attachTerminal.getMockImplementation();
    manager.attachTerminal.mockImplementation((...args) => socket.attachTerminal(...args));
    try {
      panel.toggle(); await flush();
      harness.starts[0](); await flush();
      expect(socket._connectWaiters).toHaveLength(1);
      panel.toggle(); await flush();
      expect(socket._connectWaiters).toHaveLength(0);
      expect(socket._terms.size).toBe(0);
      socket._connected = true;
      for (const waiter of socket._connectWaiters.splice(0)) waiter.resolve();
      await flush();
      expect(dispatch).not.toHaveBeenCalled();
      expect(socket._terms.size).toBe(0);
    } finally {
      socket.close();
      manager.attachTerminal.mockImplementation(originalAttach);
    }
  });

  it("settles a connected status without continuously subscribing again", async () => {
    harness.status = "connected";
    for (const listener of harness.statusListeners) listener("connected");
    await flush();
    const settledCount = harness.subscriptions;
    await flush();
    expect(harness.subscriptions).toBe(settledCount);
  });

  it("does not retain a registration from a pane that finishes mounting after collapse", async () => {
    panel.toggle(); await flush();
    expect(harness.starts).toHaveLength(1);
    panel.toggle(); await flush();
    const signal = manager.detach.mock.calls.at(-1)[1];
    expect(signal).toBeInstanceOf(AbortSignal);
    expect(signal.aborted).toBe(true);
    expect(manager.detach).toHaveBeenCalledWith("term-1", signal);
    harness.starts[0](); await flush();
    expect(harness.registered.size).toBe(0);
  });

  it("does not retain a removed source when its pane finishes loading later", async () => {
    panel.toggle(); await flush();
    await writeCached({ deviceId: "dev-1", entityId: "", kind: "projects" }, [{ project_id: "p1", sources: [] }]);
    await flush();
    harness.starts[0](); await flush();
    expect(harness.registered.size).toBe(0);
  });
});
