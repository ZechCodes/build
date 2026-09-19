// @vitest-environment jsdom
// What wakes the two pages that hold no records of their own.
//
// The archive and the capture decision were not in the cache-first brief, so
// neither keeps a record it can paint from: both re-read their machines when a
// board moves. A sync pass writes the board record and then every row on it —
// a dozen announcements for one pass — and a page that read on each of them
// would read a dozen times, each read a round trip per machine. So the wake is
// coalesced onto a frame and guarded while a read is in flight.

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache;
let subscribeBoardWrites;
let unwatch = null;

/** A frame, and the turns behind it: the wake is scheduled on one, and what
 *  it starts settles on the turns after. */
const frame = () => new Promise((done) => requestAnimationFrame(() => setTimeout(done, 0)));
const settle = async () => {
  for (let turn = 0; turn < 3; turn += 1) await frame();
};

const writeRow = (entityId) => cache.writeCached({ deviceId: "dev-1", entityId, kind: "row" }, { entityId });

beforeEach(async () => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  ({ subscribeBoardWrites } = await import("../src/core/feedRows.js"));
});

afterEach(() => {
  if (unwatch) unwatch();
  unwatch = null;
});

describe("the wake behind a board write", () => {
  it("reads once for a pass that writes a board and every row on it", async () => {
    const loads = [];
    unwatch = subscribeBoardWrites(() => {
      loads.push("read");
    });

    await cache.writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, { items: [] });
    for (const entityId of ["run-1", "run-2", "run-3", "run-4", "run-5", "run-6"]) await writeRow(entityId);
    await settle();

    expect(loads).toHaveLength(1);
  });

  it("follows a read that is in flight with exactly one more, whatever landed under it", async () => {
    const loads = [];
    let finish = null;
    unwatch = subscribeBoardWrites(() => {
      loads.push("read");
      return new Promise((done) => {
        finish = done;
      });
    });

    await writeRow("run-1");
    await settle();
    expect(loads).toHaveLength(1);

    // Three more rows move while the first read is still out on the wire.
    for (const entityId of ["run-2", "run-3", "run-4"]) await writeRow(entityId);
    await settle();
    expect(loads, "a read in flight is not joined by another").toHaveLength(1);

    finish();
    await settle();
    expect(loads, "what landed under the read is one more read, not three").toHaveLength(2);

    finish();
    await settle();
    expect(loads, "and nothing is left queued behind that one").toHaveLength(2);
  });

  it("hears the writes that move a board and no others", async () => {
    const loads = [];
    unwatch = subscribeBoardWrites(() => {
      loads.push("read");
    });

    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" }, {});
    await settle();
    expect(loads).toHaveLength(0);

    await writeRow("run-1");
    await settle();
    expect(loads).toHaveLength(1);
  });

  it("stops when the page does", async () => {
    const loads = [];
    const stop = subscribeBoardWrites(() => {
      loads.push("read");
    });

    stop();
    await writeRow("run-1");
    await settle();

    expect(loads).toHaveLength(0);
  });
});
