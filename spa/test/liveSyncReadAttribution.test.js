import { expect, it } from "vitest";
import { instrumentReadCallsites, instrumentStorageReadCallsites, createReadAttribution } from "./browser/liveSyncReadAttribution.mjs";

it("labels direct and batched reads without wrapping declarations", () => {
  const source = 'import { readCached, readCachedMany } from "./localCache.js";\nconst one = readCached(address);\nconst many = await readCachedMany(addresses);';
  const result = instrumentReadCallsites(source, "/repo/spa/src/core/cacheSync.js");
  expect(result).toContain('__profileCacheRead("core/cacheSync.js:2", readCached, address)');
  expect(result).toContain('__profileCacheRead("core/cacheSync.js:3", readCachedMany, addresses)');
  expect(instrumentReadCallsites("export function readCached(address) {}", "/repo/spa/src/core/localCache.js")).toBe("export function readCached(address) {}");
});

it("counts addresses per callsite, kind, source phase and fixed interval", () => {
  let now = 100, phase = "initial-source";
  const attribution = createReadAttribution(() => now, () => phase, 10000);
  attribution.read("core/cacheSync.js:2", [{ kind: "row" }, { kind: "thread" }]);
  now = 10101; phase = "mounted-only";
  attribution.read("core/thread.js:3", { kind: "thread" });
  attribution.stop();
  attribution.read("after-measurement", { kind: "row" });
  expect(attribution.intervals).toEqual([
    { interval: 0, reads: 2, byKind: { row: 1, thread: 1 }, byCallsite: { "core/cacheSync.js:2": 2 }, byCallsiteKind: { "core/cacheSync.js:2|row": 1, "core/cacheSync.js:2|thread": 1 }, byPhase: { "initial-source": 2 } },
    { interval: 1, reads: 1, byKind: { thread: 1 }, byCallsite: { "core/thread.js:3": 1 }, byCallsiteKind: { "core/thread.js:3|thread": 1 }, byPhase: { "mounted-only": 1 } },
  ]);
});

it("labels canonical guard reads and mapped captures", () => {
  expect(instrumentReadCallsites("const found = guards.map(captureCachedRecord);", "/repo/spa/src/core/conversationFeedReset.js")).toContain("captureCachedRecord, address)");
  expect(instrumentStorageReadCallsites("const request = store.get(recordKey(guard.address));", "/repo/spa/src/core/localCache.js")).toContain('__profileStoreGet("core/localCache.js:1", store, recordKey(guard.address))');
});
