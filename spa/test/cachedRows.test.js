// @vitest-environment jsdom
// One device's rows in the cache: reading a route's row out of them, and the
// optimistic writes a verb makes over them.
//
// A row lives in two places — its own record, which a `state` push rewrites,
// and the board list the last pass wrote, which is the only place a row naming
// no entity is at all. A press that moves a row has to move both, and put both
// back when the bridge refuses.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cachedFeedView, feedRowTarget, patchFeedRow, removeFeedRow;
let readCached, writeCached;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ cachedFeedView, feedRowTarget, patchFeedRow, removeFeedRow } = await import("../src/core/cachedRows.js"));
  ({ readCached, writeCached } = await import("../src/core/localCache.js"));
});

const branch = (over = {}) => ({
  kind: "branch",
  project_id: "p1",
  projectKey: "dev-1/p1",
  branch: "build/login",
  entity_id: "run-1",
  deviceId: "dev-1",
  ...over,
});

const feedAddress = { deviceId: "dev-1", entityId: "", kind: "feed" };
const rowAddress = (entityId) => ({ deviceId: "dev-1", entityId, kind: "row" });

const heldFeed = async () => (await readCached(feedAddress))?.value;
const heldRow = async (entityId) => (await readCached(rowAddress(entityId)))?.value;

describe("patching a row a press has moved", () => {
  it("writes the fields onto the row's own record and onto the board list", async () => {
    await writeCached(feedAddress, { items: [branch()] });
    await writeCached(rowAddress("run-1"), branch());

    await patchFeedRow("dev-1", feedRowTarget(branch()), { dismissed: true });

    expect((await heldRow("run-1")).dismissed).toBe(true);
    expect((await heldFeed()).items[0].dismissed).toBe(true);
  });

  it("puts both back exactly as they were when the bridge refuses", async () => {
    await writeCached(feedAddress, { items: [branch({ muted: false })] });
    await writeCached(rowAddress("run-1"), branch({ muted: false }));

    const undo = await patchFeedRow("dev-1", feedRowTarget(branch()), { muted: true });
    await undo();

    expect((await heldRow("run-1")).muted).toBe(false);
    expect((await heldFeed()).items[0].muted).toBe(false);
  });

  it("reverts only the pressed field after a newer row push and board read", async () => {
    const watched = branch({ muted: false, agents: [{ id: "ag-1", watched: true }] });
    await writeCached(feedAddress, { items: [watched] });
    await writeCached(rowAddress("run-1"), watched);

    const undo = await patchFeedRow("dev-1", feedRowTarget(watched), { muted: true });
    await writeCached(feedAddress, { items: [branch({ muted: false, agents: [{ id: "ag-1", watched: false }] })] },
      { observedFeedRows: true });
    await writeCached(rowAddress("run-1"), branch({ muted: true, agents: [{ id: "ag-1", watched: false }] }));
    await undo();

    expect((await heldFeed()).items[0].agents[0].watched).toBe(false);
    expect((await heldFeed()).items[0].muted).toBe(false);
    expect((await heldRow("run-1")).agents[0].watched).toBe(false);
    expect((await heldRow("run-1")).muted).toBe(false);
  });

  // A bare checkout nobody has claimed holds no conversation, so it has no
  // entity and no record of its own. The board list is the only place it is.
  it("patches a row that names no entity in the board list alone", async () => {
    const loose = branch({ entity_id: null, branch: "build/loose" });
    await writeCached(feedAddress, { items: [loose] });

    await patchFeedRow("dev-1", feedRowTarget(loose), { dismissed: true });

    expect((await heldFeed()).items[0].dismissed).toBe(true);
  });

  it("leaves the other rows alone", async () => {
    await writeCached(feedAddress, { items: [branch(), branch({ entity_id: "run-2", branch: "build/other" })] });

    await patchFeedRow("dev-1", feedRowTarget(branch()), { dismissed: true });

    expect((await heldFeed()).items.map((item) => item.dismissed)).toEqual([true, undefined]);
  });

  it("keeps the roster's observation time when only the matching board item is patched", async () => {
    await writeCached(feedAddress, { items: [branch()], runs: [branch({ agents: [{ id: "ag-1", watched: false }] })] });
    const observed = await readCached(feedAddress);

    await patchFeedRow("dev-1", feedRowTarget(branch()), { dismissed: true });

    const rewritten = await readCached(feedAddress);
    expect(rewritten.value.items[0].dismissed).toBe(true);
    expect(rewritten.value.items[0].__cacheObserved).toEqual({ at: observed.at, order: observed.order });
    expect(rewritten.value.runs[0].__cacheObserved).toEqual({ at: observed.at, order: observed.order });
  });
});

describe("taking a finished row out", () => {
  it("drops the record and the board's line, and the view stops carrying it", async () => {
    await writeCached(feedAddress, { items: [branch()] });
    await writeCached(rowAddress("run-1"), branch());

    await removeFeedRow("dev-1", feedRowTarget(branch()));

    expect(await heldRow("run-1")).toBeUndefined();
    expect((await heldFeed()).items).toEqual([]);
    expect((await cachedFeedView("dev-1")).items).toEqual([]);
  });

  it("puts the row back when the work could not be finished", async () => {
    await writeCached(feedAddress, { items: [branch()] });
    await writeCached(rowAddress("run-1"), branch());
    const observed = await readCached(feedAddress);

    const undo = await removeFeedRow("dev-1", feedRowTarget(branch()));
    await undo();

    expect((await heldRow("run-1")).entity_id).toBe("run-1");
    expect((await heldFeed()).items.map((item) => item.entity_id)).toEqual(["run-1"]);
    expect((await heldFeed()).items[0].__cacheObserved).toEqual({ at: observed.at, order: observed.order });
  });
});
