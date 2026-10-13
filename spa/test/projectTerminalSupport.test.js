import { beforeEach, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readProjectTerminalSupport, rememberProjectTerminalSupport } from "../src/core/projectTerminalSupport.js";
import { wipeCache } from "../src/core/localCache.js";
beforeEach(async () => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  await wipeCache();
});
it("retains per-device support for a cold mount, and records a downgrade", async () => {
  expect(await readProjectTerminalSupport("d1")).toEqual({ projectSources: false });
  await rememberProjectTerminalSupport("d1", { term: { projectSources: true } });
  expect(await readProjectTerminalSupport("d1")).toEqual({ projectSources: true });
  expect(await readProjectTerminalSupport("d2")).toEqual({ projectSources: false });
  await rememberProjectTerminalSupport("d1", {});
  expect(await readProjectTerminalSupport("d1")).toEqual({ projectSources: false });
});
