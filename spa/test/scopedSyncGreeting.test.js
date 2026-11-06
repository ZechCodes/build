// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { App } from "../src/appState.js";
import { createBridgeEpoch } from "../src/core/bridgeEpoch.js";
import { adoptBridgeSelection, adoptDeviceSession, captureDeviceGreeting, greetingInFlight,
  resetDeviceContexts } from "../src/core/deviceContexts.js";
import { readCached, wipeCache } from "../src/core/localCache.js";
import { captureScopedSync, startScopedSyncRuntime, stopScopedSyncRuntime, wakeScopedSync } from "../src/core/scopedSyncRuntime.js";
import { deviceSyncScope } from "../src/core/scopedSyncRegistry.js";
import { syncCursorAddress } from "../src/core/scopedTaskSyncCache.js";
import { fakeSession } from "./deviceSessionFixture.js";

const capabilities = vi.hoisted(() => ({ sync: { device: true, projectTasks: false, task: false } }));
vi.mock("../src/core/changeEvents.js", async () => ({
  ...await vi.importActual("../src/core/changeEvents.js"),
  bridgeCapabilities: () => capabilities,
  bridgeApiVersion: () => "3.36.0",
}));

const deviceId = "greeted-device";
const project = (number, name) => ({ kind: "project", key: `proj-${number}`, revision: String(number), member: true,
  row: { id: `proj-${number}`, project_id: `proj-${number}`, path: `/projects/${name}`, name, base_branch: "main" } });
const address = (kind) => ({ deviceId, entityId: "", kind });
const projectNames = async (kind) => (await readCached(address(kind)))?.value?.map((row) => row.name);
const cursor = async () => (await readCached(syncCursorAddress(deviceId, deviceSyncScope(), "")))?.value?.cursor;
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
let context, session, request, head, rows, wireEpoch;

function greet(epoch) {
  const authority = greetingInFlight(context);
  session.bridgeEpoch.acceptGreeting({ capabilities: ["bridge.epoch"], bridge_epoch: epoch, journal_incarnation: "journal" });
  wireEpoch = epoch;
  adoptBridgeSelection(context, { version: "3.36.0" }, { capabilities }, authority);
}

function response(method) {
  const common = { status: "ok", incarnation: "sync-incarnation", bridge_epoch: wireEpoch };
  if (method === "sync.snapshot") return { ...common, cursor: `S${head}`, watermark: String(head), records: rows };
  if (method === "sync.changes") return { ...common, cursor: `C${head}`, head: String(head),
    changes: rows.map((row) => ({ ...row, operation: "upsert", seq: row.revision })) };
  throw new Error(`Unexpected sync request: ${method}`);
}

beforeEach(async () => {
  vi.stubGlobal("indexedDB", new IDBFactory());
  vi.stubGlobal("IDBKeyRange", IDBKeyRange);
  vi.stubGlobal("navigator", { locks: { request: vi.fn() } });
  await wipeCache();
  resetDeviceContexts();
  App.devices = [{ id: deviceId, status: "online" }];
  session = { ...fakeSession(deviceId), bridgeEpoch: createBridgeEpoch() };
  context = adoptDeviceSession(session);
  head = 1; rows = [project(1, "TESTING")];
  greet("epoch-one");
  request = vi.fn(async (_deviceId, method) => response(method));
});

afterEach(async () => {
  stopScopedSyncRuntime();
  resetDeviceContexts();
  await wipeCache();
  vi.unstubAllGlobals();
});

it("publishes a real greeted device snapshot and later project changes into ordinary caches", async () => {
  startScopedSyncRuntime({ ownsLock: () => true, request });
  await captureScopedSync(deviceId);

  expect(await projectNames("projects")).toEqual(["TESTING"]);
  expect(await projectNames("device-projects")).toEqual(["TESTING"]);
  expect(await cursor()).toBe("C1");

  head = 2; rows = [project(2, "FROM-ANOTHER-CLIENT")];
  await wakeScopedSync(deviceId);

  expect(await projectNames("projects")).toEqual(["TESTING", "FROM-ANOTHER-CLIENT"]);
  expect(await projectNames("device-projects")).toEqual(["TESTING", "FROM-ANOTHER-CLIENT"]);
  expect(await cursor()).toBe("C2");
  expect(request.mock.calls.every(([, method]) => method.startsWith("sync."))).toBe(true);
});

it("discards an old epoch response and publishes the replacement greeting's response", async () => {
  startScopedSyncRuntime({ ownsLock: () => true, request });
  await captureScopedSync(deviceId);
  const originalGreeting = captureDeviceGreeting(context);
  const oldAnswer = { ...response("sync.changes"), cursor: "OLD", head: "2",
    changes: [{ ...project(2, "STALE"), operation: "upsert", seq: "2" }] };
  const held = deferred();
  request.mockImplementationOnce(() => held.promise);
  const oldRead = wakeScopedSync(deviceId);
  await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(3));

  head = 3; rows = [project(3, "CURRENT")];
  greet("epoch-two");
  expect(originalGreeting.current()).toBe(false);
  await captureScopedSync(deviceId);
  held.resolve(oldAnswer);
  await oldRead;

  expect(await projectNames("projects")).toEqual(["TESTING", "CURRENT"]);
  expect(await projectNames("device-projects")).toEqual(["TESTING", "CURRENT"]);
  expect(await cursor()).toBe("C3");
});
