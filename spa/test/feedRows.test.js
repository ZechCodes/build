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

/** The frames the page has asked for and not been given yet.
 *
 *  The browser's own clock has no place in this: what is being tested is that
 *  a dozen writes inside ONE frame are one read, and against a real 16 ms
 *  frame that is a race between the writes and the clock — which under load is
 *  a test that fails for reasons the page has nothing to do with. So the frame
 *  is the test's to hand out: writes land while none is running, and `frame()`
 *  is the boundary crossing, said where the reader of the test can see it. */
let framesAsked = [];
const paintedFrame = (run) => {
  framesAsked.push(run);
  return framesAsked.length;
};

/** The turns a promise chain settles on, with no clock in them. */
const turns = async (count = 3) => {
  for (let turn = 0; turn < count; turn += 1) await new Promise((done) => setTimeout(done, 0));
};

/** One frame goes by: whatever was scheduled for it runs, and what that
 *  started settles. */
const frame = async () => {
  const due = framesAsked;
  framesAsked = [];
  for (const run of due) run(0);
  await turns();
};

const writeRow = (entityId) => cache.writeCached({ deviceId: "dev-1", entityId, kind: "row" }, { entityId });

beforeEach(async () => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  framesAsked = [];
  globalThis.requestAnimationFrame = paintedFrame;
  cache = await import("../src/core/localCache.js");
  ({ subscribeBoardWrites } = await import("../src/core/feedRows.js"));
});

afterEach(() => {
  if (unwatch) unwatch();
  unwatch = null;
  delete globalThis.requestAnimationFrame;
});

describe("the wake behind a board write", () => {
  it("reads once for a pass that writes a board and every row on it", async () => {
    const loads = [];
    unwatch = subscribeBoardWrites(() => {
      loads.push("read");
    });

    await cache.writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, { items: [] });
    for (const entityId of ["run-1", "run-2", "run-3", "run-4", "run-5", "run-6"]) await writeRow(entityId);
    await frame();

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
    await frame();
    expect(loads).toHaveLength(1);

    // Three more rows move while the first read is still out on the wire.
    for (const entityId of ["run-2", "run-3", "run-4"]) await writeRow(entityId);
    await frame();
    expect(loads, "a read in flight is not joined by another").toHaveLength(1);

    finish();
    await turns();
    await frame();
    expect(loads, "what landed under the read is one more read, not three").toHaveLength(2);

    finish();
    await turns();
    await frame();
    expect(loads, "and nothing is left queued behind that one").toHaveLength(2);
  });

  it("hears the writes that move a board and no others", async () => {
    const loads = [];
    unwatch = subscribeBoardWrites(() => {
      loads.push("read");
    });

    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" }, {});
    await frame();
    expect(loads).toHaveLength(0);

    await writeRow("run-1");
    await frame();
    expect(loads).toHaveLength(1);
  });

  it("stops when the page does", async () => {
    const loads = [];
    const stop = subscribeBoardWrites(() => {
      loads.push("read");
    });

    stop();
    await writeRow("run-1");
    await frame();

    expect(loads).toHaveLength(0);
  });
});
