/** @vitest-environment jsdom */
// #169, wired: a surface that painted keeps its DOM through a cache outage.
// Nothing here is mocked above the IndexedDB factory — the surface is the real
// settings-record watcher over the real local cache, and the outage is WebKit's
// resume: the connection closed while the page slept, and the first opens
// after it fail with "Connection to Indexed Database server lost". The re-read
// is the one a reconnect's remount or a cache announcement makes.

import { transportEpoch } from "./transportEpoch.js";
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange, forceCloseDatabase } from "fake-indexeddb";
import { createSessionRpc } from "../src/core/sessionRpc.js";
import { normalizeError } from "../src/core/bridgeApi/index.js";
import { task, columns } from "./trackerWireFixture.js";

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args) }));
vi.mock("../src/core/changeEvents.js", () => ({
  onBridgeGreeted: () => () => {},
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["tasks"] } }),
  watchChanges: () => ({ dispose: () => {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => true, reconnecting: () => true, moved: () => () => {} }),
}));
vi.mock("../src/core/deviceContexts.js", async (original) => ({
  ...await original(), contextFor: () => ({}),
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
}));

const FAST_RECOVERY = { reopenDelaysMs: [0, 1, 2, 3], openTimeoutMs: 200, restMs: 60_000 };
const address = { deviceId: "dev-1", entityId: "", kind: "settings" };

let cache;
let settings;
let visibility = "visible";

const setVisibility = (state) => {
  visibility = state;
  document.dispatchEvent(new Event("visibilitychange"));
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  visibility = "visible";
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => visibility });
  cache = await import("../src/core/localCache.js");
  settings = await import("../src/core/settingsRecords.js");
  cache.setCacheRecoveryTiming(FAST_RECOVERY);
  vi.spyOn(console, "info").mockImplementation(() => {});
  notifyError.mockClear();
  document.body.innerHTML = '<div id="agentname"></div>';
});

function heldSqlSession() {
  const envelopes = new Set();
  const closes = new Set();
  const sent = [];
  const wire = {
    send(envelope) {
      sent.push(envelope.payload);
      if (envelope.payload.method === "tasks.get") return;
      for (const deliver of envelopes) deliver({ payload: { id: envelope.payload.id, ok: true, result: {} } });
    },
    onEnvelope: (fn) => { envelopes.add(fn); return () => envelopes.delete(fn); },
    onClose: (fn) => { closes.add(fn); return () => closes.delete(fn); },
    close: () => closes.forEach((fn) => fn()),
  };
  const rpc = createSessionRpc({
    bridgeEpoch: transportEpoch(),
    transport: { encryptFrame: async ({ frameFields }) => frameFields, decryptEnvelope: async ({ envelope }) => envelope },
    sessionId: "cache-opening", sessionKeyB64: "fixture", deviceId: "dev-1", timeoutMs: 0,
  });
  rpc.rideOn(wire);
  return { rpc, wire, sent };
}

describe("a cached task during SQL sender recovery", () => {
  it.each(["carrier close", "restore reset"])("keeps the real task page painted through credit waits and %s", async (event) => {
    const tracker = await import("../src/core/trackerCache.js");
    const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
    const saved = { task: task({ id: "task-cache", title: "Cached task", body: "Cached body" }), timeline: [] };
    await tracker.writeTaskRecord("dev-1", "proj-1", "task-cache", saved);
    await tracker.writeTasksRecord("dev-1", "proj-1", tracker.tasksRecord([saved.task], columns()));
    const { rpc, wire, sent } = heldSqlSession();
    for (let index = 0; index < 8; index += 1) rpc.call("tasks.get", { task_id: `held-${index}` }).catch(() => {});
    const errors = [];
    const callRpc = vi.fn(async (...args) => {
      try { return await rpc.call(...args); }
      catch (error) { const normalized = normalizeError(error); errors.push(normalized); throw normalized; }
    });
    const host = document.querySelector("#agentname");
    const page = mountTaskPage(host, {
      deviceId: "dev-1", projectId: "proj-1", projectKey: "dev-1|proj-1", taskId: "task-cache", callRpc,
      catalog: () => ({ providers: [] }), feed: () => ({ items: [], workspaces: [] }),
    });
    try {
      await vi.waitFor(() => expect(host.textContent).toContain("Cached body"));
      await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("tasks.get", { task_id: "task-cache" }));
      expect(sent.filter((request) => request.method === "tasks.get")).toHaveLength(8);
      const body = host.querySelector(".task-page-body");
      if (event === "carrier close") wire.close();
      else rpc.resetSqlReads();
      await vi.waitFor(() => expect(errors).toHaveLength(1));
      expect(errors[0]).toMatchObject({ uncertain: true, retryable: true });
      expect(notifyError).not.toHaveBeenCalled();
      expect(host.querySelector(".task-page-body")).toBe(body);
      expect(body.textContent).toContain("Cached body");
      expect((await tracker.readTaskRecord("dev-1", "proj-1", "task-cache")).task.title).toBe("Cached task");

      await tracker.writeTaskRecord("dev-1", "proj-1", "task-cache", {
        ...saved, task: { ...saved.task, title: "Refreshed task", body: "Refreshed body" },
      });
      await vi.waitFor(() => expect(host.textContent).toContain("Refreshed body"));
      expect(notifyError).not.toHaveBeenCalled();
    } finally {
      page.dispose();
      rpc.close();
    }
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  delete document.visibilityState;
});

/** The surface: paints the record's name, and paints nothing for no record —
 *  the reader every "blank after resume" surface looks like. */
function mountSurface() {
  const host = document.querySelector("#agentname");
  return settings.watchSettingsRecord(address, (value) => {
    host.textContent = value?.name || "";
  });
}

/** The connection the cache holds, captured on its next transaction. */
async function heldConnection() {
  const handles = [];
  const originalTransaction = IDBDatabase.prototype.transaction;
  const spy = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    handles.push(this);
    return originalTransaction.apply(this, args);
  });
  await cache.readCached(address);
  spy.mockRestore();
  return handles[0];
}

/** WebKit's storage server, gone until `restore` is called. */
function storageServerGone() {
  const originalOpen = indexedDB.open.bind(indexedDB);
  let gone = true;
  const open = vi.spyOn(indexedDB, "open").mockImplementation((...args) => {
    if (!gone) return originalOpen(...args);
    const request = { error: new DOMException("Connection to Indexed Database server lost. Refresh the page to try again", "UnknownError") };
    setTimeout(() => request.onerror?.({ preventDefault() {} }));
    return request;
  });
  return { open, restore: () => { gone = false; } };
}

describe("a surface over a cache that lost its connection", () => {
  it("keeps what it painted until a read really answers, then paints what the cache holds", async () => {
    await cache.writeCached(address, { name: "Night shift" });
    const surface = mountSurface();
    await surface.whenPainted();
    const host = document.querySelector("#agentname");
    expect(host.textContent).toBe("Night shift");

    // Suspended: the connection goes, and the server with it.
    setVisibility("hidden");
    forceCloseDatabase(await heldConnection());
    const server = storageServerGone();
    setVisibility("visible");

    // The reconnect re-reads the surface. Every open fails, the cache rests,
    // and the surface still shows what it showed.
    const reread = surface.read();
    await vi.waitFor(() => expect(cache.cacheHealth().state).toBe("resting"));
    expect(host.textContent).toBe("Night shift");

    // The storage server is back; the next wake ends the rest and the read answers.
    server.restore();
    setVisibility("hidden");
    setVisibility("visible");
    await reread;
    expect(host.textContent).toBe("Night shift");
    expect(cache.cacheHealth().state).toBe("ready");

    // And the surface goes on hearing writes.
    await cache.writeCached(address, { name: "Morning shift" });
    await surface.whenPainted();
    expect(host.textContent).toBe("Morning shift");
  });

  it("spends no reopen attempts while the page is hidden", async () => {
    await cache.writeCached(address, { name: "Night shift" });
    const surface = mountSurface();
    await surface.whenPainted();
    const host = document.querySelector("#agentname");

    setVisibility("hidden");
    forceCloseDatabase(await heldConnection());
    const server = storageServerGone();
    const reread = surface.read();
    // The first attempt is made at once; the retries wait for the page.
    await vi.waitFor(() => expect(server.open).toHaveBeenCalledTimes(1));
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(server.open).toHaveBeenCalledTimes(1);
    expect(host.textContent).toBe("Night shift");

    server.restore();
    setVisibility("visible");
    await reread;
    expect(server.open).toHaveBeenCalledTimes(2);
    expect(host.textContent).toBe("Night shift");
    const lost = (await import("../src/core/connectionDiagnostics.js")).connectionDiagnosticHistory()
      .find((entry) => entry.event === "cache-connection-lost");
    expect(lost).toMatchObject({ visibility: "hidden", error: "UnknownError" });
  });
});
