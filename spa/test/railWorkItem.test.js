// What a rail reads, on its own: which row a route stands on, and the watches
// that tell it the answer moved.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { createRailWorkItem } = await import("../src/core/railWorkItem.js");
const { wipeCache, writeCached } = await import("../src/core/localCache.js");

const DEVICE = "dev-1";

const cacheScope = {
  deviceId: DEVICE,
  active: () => true,
  address: (parts) => ({ ...parts, deviceId: DEVICE }),
};

const branchContext = {
  kind: "branch",
  projectId: "p1",
  branch: "build/login",
  feedRoute: () => ({ name: "branch", deviceId: DEVICE, projectId: "p1", branch: "build/login" }),
};

const flush = async () => {
  for (let turn = 0; turn < 16; turn += 1) await new Promise((done) => setTimeout(done, 0));
};

const rowRecord = (id) => ({
  kind: "branch",
  project_id: "p1",
  projectKey: `${DEVICE}/p1`,
  deviceId: DEVICE,
  branch: `build/${id}`,
  run_id: id,
  agents: [],
});

const writeRow = (id) => writeCached({ deviceId: DEVICE, entityId: id, kind: "row", sub: "" }, rowRecord(id));

let records;
let reread;

beforeEach(async () => {
  await wipeCache();
  reread = vi.fn(async () => {});
  records = createRailWorkItem({
    context: { kind: "branch", deviceId: DEVICE },
    railContext: branchContext,
    cacheScope,
    callFor: () => async () => ({}),
    named: () => {},
    standOn: () => {},
    reread: () => reread(),
    redrawConversation: () => {},
    alive: () => true,
  });
});

afterEach(() => records.unwatch());

describe("a rail waiting for a row of its own", () => {
  // The device-wide watch is the fallback for a route this machine holds no row
  // for yet. Resolving a route walks every row the device has, so one re-read
  // per row written would cost N full board reads for the N rows a boot pass
  // writes — on exactly the path the cache is supposed to make fast.
  it("answers a burst of rows with one re-read and one after it", async () => {
    // A re-read costs a cache round trip — resolving a route reads every row
    // address on the device and the two lists beside them — and here it is held
    // open, because what collapses a burst is the read still being in flight
    // when the next row lands.
    let finish;
    reread.mockImplementation(() => new Promise((done) => {
      finish = done;
    }));
    records.watch(null);

    await Promise.all(["run-1", "run-2", "run-3", "run-4", "run-5"].map(writeRow));
    await flush();
    expect(reread).toHaveBeenCalledTimes(1);

    finish();
    await flush();

    // One more, for everything that moved while the first was out.
    expect(reread).toHaveBeenCalledTimes(2);
    finish();
    await flush();
    expect(reread).toHaveBeenCalledTimes(2);
  });

  // …and it is still a watch: a row written after everything settled is heard.
  it("hears a row that arrives once the burst is over", async () => {
    records.watch(null);
    await writeRow("run-1");
    await flush();
    reread.mockClear();

    await writeRow("run-2");
    await flush();

    expect(reread).toHaveBeenCalledTimes(1);
  });

  // A record that is not a row says nothing about which row this route is on.
  it("ignores a write that is not a row", async () => {
    records.watch(null);
    await flush();
    reread.mockClear();

    await writeCached({ deviceId: DEVICE, entityId: "run-1", kind: "status", sub: "" }, { head: "abc" });
    await flush();

    expect(reread).not.toHaveBeenCalled();
  });
});
