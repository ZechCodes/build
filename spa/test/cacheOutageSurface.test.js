/** @vitest-environment jsdom */
// #169, wired: a surface that painted keeps its DOM through a cache outage.
// Nothing here is mocked above the IndexedDB factory — the surface is the real
// settings-record watcher over the real local cache, and the outage is WebKit's
// resume: the connection closed while the page slept, and the first opens
// after it fail with "Connection to Indexed Database server lost". The re-read
// is the one a reconnect's remount or a cache announcement makes.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange, forceCloseDatabase } from "fake-indexeddb";

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
  document.body.innerHTML = '<div id="agentname"></div>';
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
