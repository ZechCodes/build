// @vitest-environment jsdom
// The task surface's records.
//
// Tasks left the board, so no pass fills them and no push carries their
// bodies: the surface reads on demand and writes what it read, and every paint
// after that is the record's. What says a record is behind is a `state` push
// naming the task — or the mount, which is the only other moment anything
// could tell this surface that the task moved while the tab was shut.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let taskAddress, readTaskRecord, forgetTaskRecords, taskRecordsHeld;
let readCached, writeCached;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ taskAddress, readTaskRecord, forgetTaskRecords, taskRecordsHeld } = await import("../src/core/taskCache.js"));
  ({ readCached, writeCached } = await import("../src/core/localCache.js"));
});

const held = async (sub) => (await readCached(taskAddress("dev-1", "task-1", sub)))?.value;

const readThrough = (sub, read, options) =>
  readTaskRecord({ deviceId: "dev-1", taskId: "task-1", sub, read, ...options });

describe("reading a task record", () => {
  it("reads the wire once and answers the record after that", async () => {
    const read = vi.fn(async () => ({ goal: "ship it" }));

    expect(await readThrough("get", read)).toEqual({ goal: "ship it" });
    expect(await readThrough("get", read)).toEqual({ goal: "ship it" });

    expect(read).toHaveBeenCalledTimes(1);
    expect(await held("get")).toEqual({ goal: "ship it" });
  });

  it("reads again when the row it is about has moved", async () => {
    const read = vi.fn(async () => ({ goal: "ship it" }));
    await readThrough("get", read);

    await readThrough("get", read, { force: true });

    expect(read).toHaveBeenCalledTimes(2);
  });

  it("keeps the record it holds when a read fails, and says the read failed", async () => {
    await writeCached(taskAddress("dev-1", "task-1", "stages"), { stages: [{ id: "s1" }] });
    const read = vi.fn(async () => {
      throw new Error("offline");
    });

    await expect(readThrough("stages", read, { force: true })).rejects.toThrow("offline");

    expect(await held("stages")).toEqual({ stages: [{ id: "s1" }] });
  });

  it("does not let a late pull overwrite a newer cache write", async () => {
    let answer;
    const pending = readThrough("get", () => new Promise((resolve) => { answer = resolve; }));
    await new Promise((done) => setTimeout(done, 0));
    await writeCached(taskAddress("dev-1", "task-1", "get"), { goal: "newer announcement" });
    answer({ goal: "late pull" });

    expect(await pending).toEqual({ goal: "newer announcement" });
    expect(await held("get")).toEqual({ goal: "newer announcement" });
  });

  it("raises a cold read that fails — there is nothing to paint", async () => {
    const read = async () => {
      throw new Error("unknown task_id");
    };

    await expect(readThrough("get", read)).rejects.toThrow("unknown task_id");
  });

  it("holds each sub-key on its own", async () => {
    await readThrough("stages", async () => ({ stages: [] }));
    await readThrough("stage:s1", async () => ({ stage_id: "s1", contents: "# Wire" }));

    expect(await held("stages")).toEqual({ stages: [] });
    expect((await held("stage:s1")).contents).toBe("# Wire");
  });

  // The mount asks this to know which frame it is about to paint: the
  // records', with the machine read behind it, or the machine's.
  it("says whether the records a mount paints from are all there", async () => {
    expect(await taskRecordsHeld("dev-1", "task-1", ["get", "stages"])).toBe(false);

    await readThrough("get", async () => ({ goal: "ship it" }));
    expect(await taskRecordsHeld("dev-1", "task-1", ["get", "stages"])).toBe(false);

    await readThrough("stages", async () => ({ stages: [] }));
    expect(await taskRecordsHeld("dev-1", "task-1", ["get", "stages"])).toBe(true);
  });

  it("lets go of everything one task holds", async () => {
    await readThrough("get", async () => ({ goal: "ship it" }));
    await readThrough("stages", async () => ({ stages: [] }));

    await forgetTaskRecords("dev-1", "task-1");

    expect(await held("get")).toBeUndefined();
    expect(await held("stages")).toBeUndefined();
  });
});
