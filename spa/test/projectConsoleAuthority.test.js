// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const harness = vi.hoisted(() => ({ registered: new Map(), starts: [], status: null, statusListeners: new Set(), subscriptions: 0 }));
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
const { deleteCached, readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
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

  it("hides a sole cached source while keeping it selected for new shells", async () => {
    expect(getComputedStyle(picker()).visibility).toBe("hidden");
    expect(picker().value).toBe("code");
    document.querySelector(".console-new").click(); await flush();
    expect(manager.createTerminal).toHaveBeenCalledWith({ project_id: "p1", source_id: "code", source_path: "/code" }, 80, 24);
  });

  it("also hides the picker for a legacy project's single path", async () => {
    await writeCached(projectsAddress, [{ project_id: "p1", name: "Code", path: "/code" }]);
    await flush();
    expect(getComputedStyle(picker()).visibility).toBe("hidden");
    expect(picker().value).toBe("primary");
  });

  it("keeps an empty source picker disabled", async () => {
    await writeCached(projectsAddress, [{ project_id: "p1", sources: [] }]);
    await flush();
    expect(picker().disabled).toBe(true);
    expect(document.querySelector(".console-new").disabled).toBe(true);
  });
});

describe("late project console roster responses", () => {
  it("retries a failed missing roster on a later cache update", async () => {
    manager.listTerminals.mockRejectedValueOnce(new Error("socket lost"))
      .mockResolvedValueOnce([{ term_id: "retried-term" }]);
    await deleteCached([rosterAddress]); await flush();
    expect(manager.listTerminals).toHaveBeenCalledTimes(1);
    expect(await readCached(rosterAddress)).toBeUndefined();
    await writeCached({ deviceId: "dev-1", entityId: "unrelated", kind: "tasks" }, []);
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
