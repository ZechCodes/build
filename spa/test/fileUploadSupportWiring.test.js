/** @vitest-environment jsdom */
import { afterEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
vi.doUnmock("../src/core/changeEvents.js");
vi.doUnmock("../src/core/deviceContexts.js");
vi.resetModules();
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { greetBridge, resetChangeEvents, bridgeCapabilities } = await import("../src/core/changeEvents.js");
const { readFileUploadSupport, fileUploadSupportAddress } = await import("../src/core/fileUploadSupport.js");
const { subscribeCache } = await import("../src/core/localCache.js");
afterEach(resetChangeEvents);
it("greetings refresh the cached flags watched by a mounted Files view", async () => {
  const changed = vi.fn();
  const stop = subscribeCache(fileUploadSupportAddress("upload-device"), changed);
  const greet = (capabilities) => greetBridge(async (method) => method === "session.hello" ? { api_version: "3.13.0", capabilities } : {}, { deviceId: "upload-device", strict: true });
  expect(bridgeCapabilities("upload-device").fs).toMatchObject({ uploads: false, createDirectory: false });
  await greet(["fs.uploadBegin", "fs.createDirectory"]);
  await vi.waitFor(async () => expect(await readFileUploadSupport("upload-device")).toEqual({ uploads: true, createDirectory: true }));
  expect(changed).toHaveBeenCalled();
  await greet([]);
  await vi.waitFor(async () => expect(await readFileUploadSupport("upload-device")).toEqual({ uploads: false, createDirectory: false }));
  stop();
});
