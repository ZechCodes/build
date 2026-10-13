import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

// Independent module graphs represent open tabs. They share only IndexedDB
// and the existing cache channel; each graph owns its own capability memory.
let ports, messages, observing, listening, cache;
class TabChannel {
  constructor(name) { this.name = name; ports.add(this); }
  postMessage(data) {
    messages.push({ name: this.name, data });
    for (const port of ports) {
      if (port !== this && port.name === this.name) {
        queueMicrotask(() => port.onmessage?.({ data: structuredClone(data) }));
      }
    }
  }
  unref() {}
  close() { ports.delete(this); }
}

beforeEach(async () => {
  ports = new Set(); messages = [];
  vi.stubGlobal("BroadcastChannel", TabChannel);
  vi.stubGlobal("indexedDB", new IDBFactory());
  vi.stubGlobal("IDBKeyRange", IDBKeyRange);
  vi.resetModules();
  observing = await import("../src/core/taskWaitingOnSupport.js");
  cache = await import("../src/core/localCache.js");
  vi.resetModules();
  listening = await import("../src/core/taskWaitingOnSupport.js");
});
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); ports.clear(); });

it("shares refused capable-history evidence with an open tab before an incapable greeting", async () => {
  const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(() => {
    throw new DOMException("History storage refused", "QuotaExceededError");
  });
  expect(await observing.rememberTaskWaitingOnSupport("shared", { tasks: { waitingOn: true } })).toBe(false);
  expect(await cache.readCached(observing.taskWaitingOnSupportAddress("shared"))).toBeUndefined();
  await vi.waitFor(async () => expect(await listening.readTaskWaitingOnEverSupported("shared")).toBe(true));
  expect(await listening.readTaskWaitingOnEverSupported("other")).toBe(false);
  expect(await listening.readTaskWaitingOnSupport("shared")).toBe(false);
  expect(messages.some(({ name, data }) => name === "build-cache" && data.evidence?.waitingOnSeen === true)).toBe(true);
  const count = messages.length;
  await Promise.resolve();
  expect(messages).toHaveLength(count); // receiving evidence never echoes it
  put.mockRestore();
  await listening.rememberTaskWaitingOnSupport("shared", {});
  expect((await cache.readCached(observing.taskWaitingOnSupportAddress("shared"))).value)
    .toEqual({ waitingOn: false, everWaitingOn: true });
});

it("shares evidence even without IndexedDB, without granting rendering support", async () => {
  vi.stubGlobal("indexedDB", undefined);
  const pending = observing.rememberTaskWaitingOnSupport("pending", { tasks: { waitingOn: true } }, () => false);
  await vi.waitFor(async () => expect(await listening.readTaskWaitingOnEverSupported("pending")).toBe(true));
  expect(await pending).toBe(false);
  expect(await listening.readTaskWaitingOnSupport("pending")).toBe(false);
});

it("still remembers and retries locally when BroadcastChannel is unavailable", async () => {
  vi.stubGlobal("BroadcastChannel", undefined);
  vi.resetModules();
  const local = await import("../src/core/taskWaitingOnSupport.js");
  const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementationOnce(() => {
    throw new DOMException("Refused first put", "QuotaExceededError");
  });
  expect(await local.rememberTaskWaitingOnSupport("local", { tasks: { waitingOn: true } })).toBe(false);
  expect(await local.readTaskWaitingOnEverSupported("local")).toBe(true);
  put.mockRestore();
  expect(await local.rememberTaskWaitingOnSupport("local", { tasks: { waitingOn: true } })).toBe(true);
});
