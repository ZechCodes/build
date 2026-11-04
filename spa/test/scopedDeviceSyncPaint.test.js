// @vitest-environment jsdom
// A registry fence is invisible to readers: the real mounted rail and project
// directory continue reading their ordinary cache records throughout recovery.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { deviceRecords, deviceRecord } from "./fixtures/deviceSync.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const deviceId = "device-paint-997";
const scope = { kind: "device" };
const address = (kind) => ({ deviceId, entityId: "", kind });
const project = (name) => ({ ...deviceRecords().find((record) => record.kind === "project").row, name });
const reply = (extra) => ({ status: "ok", scope, incarnation: "incarnation-paint", bridge_epoch: "epoch-paint", ...extra });
const reconciling = () => Object.assign(new Error("sync_reconciling"), { code: "unavailable", retryable: true });
const tick = async () => { for (let count = 0; count < 12; count += 1) await new Promise(setImmediate); };
let cache, feed, inbox, contexts, engine;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = bodyHtml;
  const { App } = await import("../src/app.js");
  Object.assign(App, { route: { name: "inbox" }, devices: [{ id: deviceId, name: "Synthetic device", status: "online" }],
    selectedDeviceId: deviceId, deviceFilter: null });
  cache = await import("../src/core/localCache.js");
  feed = await import("../src/core/taskFeed.js");
  inbox = await import("../src/core/inboxView.js");
  contexts = await import("../src/core/deviceContexts.js");
});

afterEach(() => {
  engine?.dispose(); engine = null;
  inbox?.unmountInboxList(); feed?.stopFeed(); feed?.dropFeedDevice(deviceId);
  contexts?.resetDeviceContexts();
  document.body.innerHTML = "";
  vi.useRealTimers();
});

async function mountCachedRows() {
  const { writeRailBoard } = await import("./railCacheFixture.js");
  await writeRailBoard({ projects: [project("Cached project")], workspaces: [], items: [] }, { deviceId });
  await cache.writeCached(address("device-projects"), [project("Cached project")]);
  inbox.setInboxView("inbox"); inbox.mountInboxList(); await feed.startFeed();
  const { deviceProjectsPanelHtml, mountDeviceProjects } = await import("../src/views/deviceProjects.js");
  const host = document.createElement("section");
  host.id = "device-directory"; host.innerHTML = deviceProjectsPanelHtml(); document.body.appendChild(host);
  // A mounted panel's independent read remains outstanding; its cached rows
  // must paint before this request, or any device-scope request, answers.
  void mountDeviceProjects(host, { deviceId, deviceName: "Synthetic device", callRpc: () => new Promise(() => {}) });
  await vi.waitFor(() => {
    expect(document.querySelector("#inbox-list .stitle")?.textContent).toBe("Cached project");
    expect(host.querySelector(".pname")?.textContent).toBe("Cached project");
  });
  return host;
}

async function startEngine(request, onError) {
  const { createScopedSyncEngine } = await import("../src/core/scopedSyncEngine.js");
  const { createScopedDeviceSyncCache } = await import("../src/core/scopedDeviceSyncCache.js");
  engine = createScopedSyncEngine({ request, onError, epoch: () => "epoch-paint",
    storeFor: () => createScopedDeviceSyncCache({ deviceId, scope, bridgeIdentity: deviceId }) });
  engine.track(scope);
  return engine.catchUp(scope);
}

it("keeps cached inbox and directory DOM and records identical through a long fence, then paints catch-up", async () => {
  const host = await mountCachedRows();
  const beforeDom = document.querySelector("#inbox-list").innerHTML;
  const beforeDirectory = host.innerHTML;
  const kinds = ["projects", "device-projects", "feed"];
  const beforeCache = await Promise.all(kinds.map((kind) => cache.readCached(address(kind))));
  let fenced = true;
  const onError = vi.fn();
  const request = vi.fn(async (method) => {
    if (fenced) throw reconciling();
    return method === "sync.snapshot" ? reply({ consistency: "leased", lease: { id: "lease-paint", expires_at_ms: Date.now() + 60_000 },
      watermark: "1", cursor: "cursor-paint-1", records: [deviceRecord("project", "proj-1", project("Caught-up project"))] })
      : reply({ head: "1", cursor: "cursor-paint-1", changes: [] });
  });
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  expect(await startEngine(request, onError)).toBe(false);
  await vi.advanceTimersByTimeAsync(1000); await tick();
  await vi.advanceTimersByTimeAsync(2000); await tick();
  expect(request).toHaveBeenCalledTimes(3);
  expect(onError).not.toHaveBeenCalled();
  expect(await Promise.all(kinds.map((kind) => cache.readCached(address(kind))))).toEqual(beforeCache);
  expect(document.querySelector("#inbox-list").innerHTML).toBe(beforeDom);
  expect(host.innerHTML).toBe(beforeDirectory);
  expect(document.body.textContent).not.toMatch(/sync_reconciling|reconciling|No projects yet|No activity/i);
  fenced = false;
  await vi.advanceTimersByTimeAsync(5000); await tick();
  vi.useRealTimers();
  await vi.waitFor(() => {
    expect(document.querySelector("#inbox-list .stitle")?.textContent).toBe("Caught-up project");
    expect(host.querySelector(".pname")?.textContent).toBe("Caught-up project");
  });
  expect(onError).not.toHaveBeenCalled();
});

it("reports a persisting genuine failure through the engine's existing error callback", async () => {
  await mountCachedRows();
  const failure = Object.assign(new Error("Synthetic device read failure"), { code: "unavailable", retryable: true });
  const request = vi.fn(async () => { throw failure; });
  const onError = vi.fn();
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  expect(await startEngine(request, onError)).toBe(false);
  expect(onError).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1000); await tick();
  expect(onError).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(2000); await tick();
  expect(request).toHaveBeenCalledTimes(3);
  expect(onError).toHaveBeenCalledWith(failure);
  expect(document.querySelector("#inbox-list .stitle")?.textContent).toBe("Cached project");
});
