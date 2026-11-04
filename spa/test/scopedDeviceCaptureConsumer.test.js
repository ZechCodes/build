import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { deviceRecords } from "./fixtures/deviceSync.js";

const deviceId = "device-capture-consumer-997", scope = { kind: "device" };
const address = (kind) => ({ deviceId, entityId: "", kind });
const reply = (extra) => ({ status: "ok", scope, incarnation: "capture-incarnation", bridge_epoch: "capture-epoch", ...extra });
let cache, inbox, engine;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  inbox = await import("../src/core/inbox.js");
});

afterEach(() => { engine?.dispose(); engine = null; });

async function coldCapture(fields) {
  const fixture = deviceRecords().find((record) => record.kind === "inbox_capture");
  const record = { ...fixture, row: { ...fixture.row, ...fields } };
  const request = vi.fn(async (method) => method === "sync.snapshot"
    ? reply({ consistency: "leased", lease: { id: "capture-lease", expires_at_ms: Date.now() + 60_000 },
      watermark: "1", cursor: "capture-cursor", records: [record] })
    : reply({ head: "1", cursor: "capture-cursor", changes: [] }));
  const { createScopedSyncEngine } = await import("../src/core/scopedSyncEngine.js");
  const { createScopedDeviceSyncCache } = await import("../src/core/scopedDeviceSyncCache.js");
  engine = createScopedSyncEngine({ request, epoch: () => "capture-epoch",
    storeFor: () => createScopedDeviceSyncCache({ deviceId, scope, bridgeIdentity: deviceId }) });
  engine.track(scope);
  expect(await engine.catchUp(scope)).toBe(true);
  expect(request.mock.calls.map(([method]) => method)).toEqual(["sync.snapshot", "sync.changes"]);
  // The consumer reads its ordinary feed cache. The journal summary does not
  // provide capture.get's question text, user text or full routing body.
  const items = (await cache.readCached(address("feed"))).value.items;
  expect(items).toHaveLength(1);
  expect(items[0].question).toBeUndefined();
  expect(items[0].text).toBeUndefined();
  const entries = inbox.captureEntries(items);
  expect(entries).toHaveLength(1);
  return entries[0];
}

it("a cold engine capture needing an answer says who it is waiting on without inventing question text", async () => {
  const entry = await coldCapture({ state: "unrouted", needs_answer: true });
  expect(inbox.captureStatusText(entry)).toBe("Waiting for your answer");
  expect(entry.question).toBe("");
  expect(entry.text).toBe("");
  expect(entry.working).toBe(false);
  const html = inbox.captureRowHtml(entry);
  expect(html).toContain("Waiting for your answer");
  expect(html).not.toContain("inbox-reason");
  expect(html).not.toContain("inbox-status-running");
});

it("a cold engine routing capture retains its running activity through the actual cache consumer", async () => {
  const entry = await coldCapture({ state: "routing", needs_answer: false });
  expect(entry.working).toBe(true);
  expect(inbox.captureStatusText(entry)).toBe("Deciding where this goes");
  expect(inbox.captureRowHtml(entry)).toContain('class="inbox-status-dot inbox-status-running"');
  expect(entry.question).toBe("");
  expect(entry.text).toBe("");
});
