/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { createModelCatalog } from "../src/core/modelCatalog.js";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { deviceModelsAddress } from "../src/core/settingsRecords.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const offering = (id) => ({ providers: [{ id, label: id, models: [], efforts: [] }] });
const address = deviceModelsAddress("dev-a");
let catalog;
beforeEach(wipeCache);
afterEach(() => catalog?.disposeModelCatalog());

it("rechecks the answer's authority at the cache write, and reasks after supersession", async () => {
  await writeCached(address, offering("cached"));
  const answers = [];
  const context = {
    deviceId: "dev-a", active: () => true,
    rpc: vi.fn(() => new Promise((resolve) => answers.push(resolve))),
  };
  let latest = 1;
  let supersessionQueued = false;
  catalog = createModelCatalog(context, {
    whenGreeted: async (dispatch) => {
      const mine = latest;
      return {
        sent: dispatch(),
        stands: () => {
          // The first answer stands when read from the RPC. A new greeting
          // starts in the async handoff from that reader to the cache writer.
          if (!supersessionQueued) {
            supersessionQueued = true;
            queueMicrotask(() => { latest += 1; });
          }
          return latest === mine;
        },
      };
    },
  });
  expect((await catalog.modelCatalog()).providers[0].id).toBe("cached");
  await vi.waitFor(() => expect(answers).toHaveLength(1));
  answers[0](offering("superseded"));
  await vi.waitFor(() => expect(answers).toHaveLength(2));
  expect((await readCached(address)).value.providers[0].id).toBe("cached");
  answers[1](offering("current"));
  await vi.waitFor(async () => expect((await readCached(address)).value.providers[0].id).toBe("current"));
  expect((await catalog.modelCatalog()).providers[0].id).toBe("current");
});
