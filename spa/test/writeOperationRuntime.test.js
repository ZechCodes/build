// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const accountId = "592ab99a-62d1-4c17-8c95-3148fcbb6726";
let runtime;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.head.innerHTML = `<meta name="build-account-id" content="${accountId}">`;
  runtime = await import("../src/core/writeOperationRuntime.js");
});
afterEach(async () => { await runtime?.resetWriteOperationRuntime(); });

describe("pending write boot authority", () => {
  it("requires a server-provided UUID rather than a placeholder or missing identity", () => {
    expect(runtime.writeAccountId()).toBe(accountId);
    document.head.innerHTML = '<meta name="build-account-id" content="{{ACCOUNT_ID}}">';
    expect(runtime.deviceWriteOperations("device-A")).toBeNull();
    document.head.innerHTML = "";
    expect(runtime.writeAccountId()).toBeNull();
  });

  it("restores accepted projections before any greeting and isolates the account", async () => {
    const { createWriteOperationCache } = await import("../src/core/writeOperationCache.js");
    const { readCached, writeCached } = await import("../src/core/localCache.js");
    const address = { deviceId: "device-A", entityId: "task-A", kind: "test" };
    const cache = createWriteOperationCache({ accountId, deviceId: "device-A", principalId: `user:${accountId}` });
    await writeCached(address, { title: "before" });
    await cache.put({ operationId: "operation-A", revision: 1, state: "accepted", acknowledged: true,
      overlayActive: true, overlay: [{ address, kind: "merge", value: { title: "accepted" } }] });
    await runtime.restoreWriteOperations([{ id: "device-A" }]);
    expect((await readCached(address)).value.title).toBe("accepted");
    await runtime.resetWriteOperationRuntime();
    document.head.innerHTML = '<meta name="build-account-id" content="73ea8352-5c43-431c-b31d-0ab56a811b31">';
    await runtime.restoreWriteOperations([{ id: "device-A" }]);
    expect((await readCached(address)).value.title).toBe("before");
  });
});
