// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const fixture = (name) => JSON.parse(readFileSync(resolve("../fixtures/api/v1", `${name}.json`), "utf8"));

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

it("caches only update status fields from an authenticated push envelope", async () => {
  const { dispatchChangeEvent } = await import("../src/core/changeEvents.js");
  const { bridgeUpdateAddress } = await import("../src/core/bridgeUpdates.js");
  const { readCached, wipeCache } = await import("../src/core/localCache.js");
  await wipeCache();
  const status = fixture("bridge.update_status").result;
  const event = { ...status, type: "bridge.update_status", bridge_epoch: fixture("session.hello").result.bridge_epoch };
  expect(dispatchChangeEvent(event, "push-payload-device")).toBe(true);
  await expect.poll(async () => (await readCached(bridgeUpdateAddress("push-payload-device")))?.value).toEqual(status);
});
