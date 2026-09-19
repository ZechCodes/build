// @vitest-environment jsdom
// The issue surface's records.
//
// Issues left the board, so no pass fills them and no push carries their
// bodies: the surface reads on demand and writes what it read, and every paint
// after that is the record's. What says a record is behind is a `state` push
// naming the issue — or the mount, which is the only other moment anything
// could tell this surface that the issue moved while the tab was shut.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let issueAddress, readIssueRecord, forgetIssueRecords, issueRecordsHeld;
let readCached, writeCached;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ issueAddress, readIssueRecord, forgetIssueRecords, issueRecordsHeld } = await import("../src/core/issueCache.js"));
  ({ readCached, writeCached } = await import("../src/core/localCache.js"));
});

const held = async (sub) => (await readCached(issueAddress("dev-1", "issue-1", sub)))?.value;

const readThrough = (sub, read, options) =>
  readIssueRecord({ deviceId: "dev-1", issueId: "issue-1", sub, read, ...options });

describe("reading an issue record", () => {
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
    await writeCached(issueAddress("dev-1", "issue-1", "stages"), { stages: [{ id: "s1" }] });
    const read = vi.fn(async () => {
      throw new Error("offline");
    });

    await expect(readThrough("stages", read, { force: true })).rejects.toThrow("offline");

    expect(await held("stages")).toEqual({ stages: [{ id: "s1" }] });
  });

  it("raises a cold read that fails — there is nothing to paint", async () => {
    const read = async () => {
      throw new Error("unknown issue_id");
    };

    await expect(readThrough("get", read)).rejects.toThrow("unknown issue_id");
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
    expect(await issueRecordsHeld("dev-1", "issue-1", ["get", "stages"])).toBe(false);

    await readThrough("get", async () => ({ goal: "ship it" }));
    expect(await issueRecordsHeld("dev-1", "issue-1", ["get", "stages"])).toBe(false);

    await readThrough("stages", async () => ({ stages: [] }));
    expect(await issueRecordsHeld("dev-1", "issue-1", ["get", "stages"])).toBe(true);
  });

  it("lets go of everything one issue holds", async () => {
    await readThrough("get", async () => ({ goal: "ship it" }));
    await readThrough("stages", async () => ({ stages: [] }));

    await forgetIssueRecords("dev-1", "issue-1");

    expect(await held("get")).toBeUndefined();
    expect(await held("stages")).toBeUndefined();
  });
});
