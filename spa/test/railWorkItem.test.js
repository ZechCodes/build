// What a rail reads, on its own: which row a route stands on, and the watches
// that tell it the answer moved.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { createRailWorkItem } = await import("../src/core/railWorkItem.js");
const { createConversationCache } = await import("../src/core/conversationCache.js");
const { createThreadCache } = await import("../src/core/thread.js");
const { wipeCache, writeCached } = await import("../src/core/localCache.js");
const { writeRailBoard, writeRailThread, writeRailWorkItem } = await import("./railCacheFixture.js");

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

describe("a workspace whose cached records arrive independently", () => {
  it("opens the cached thread from the workspace owner before the roster row arrives, then hears that row", async () => {
    records.unwatch();
    await writeRailBoard({
      projects: [{ project_id: "p1", name: "build" }],
      workspaces: [{ id: "ws-1", project_id: "p1", name: "login", entity_id: "run-3" }],
    });
    await writeRailThread("run-3", "ag-remembered", {
      items: [{ type: "message", data: { sequence: 1, role: "agent", body: "cached before roster" } }],
    });
    const standing = [];
    const workspaceContext = {
      kind: "workspace",
      projectId: "p1",
      workspaceId: "ws-1",
      feedRoute: () => ({ name: "workspace", deviceId: DEVICE, projectId: "p1", workspaceId: "ws-1" }),
    };
    records = createRailWorkItem({
      context: { kind: "workspace", deviceId: DEVICE },
      railContext: workspaceContext,
      cacheScope,
      callFor: () => async () => ({}),
      named: () => {},
      standOn: (row) => standing.push(row),
      reread: () => {},
      redrawConversation: () => {},
      alive: () => true,
    });

    const entityId = await records.entityIdFor(workspaceContext);
    expect(entityId).toBe("run-3");
    expect(await records.read(entityId)).toBeNull();

    const threadCache = createThreadCache();
    const conversation = createConversationCache({
      addressOf: () => ({ deviceId: DEVICE, entityId, agentId: "ag-remembered", conversationId: "ag-remembered" }),
      threadCache,
      onThreadSeeded: () => {},
      onSurfacesSeeded: () => {},
    });
    expect(await conversation.seed()).toBe(true);
    expect(threadCache.readWindow().items[0].data.body).toBe("cached before roster");

    records.watch(entityId);
    await writeRailWorkItem({
      kind: "branch",
      project_id: "p1",
      workspace_id: "ws-1",
      run_id: "run-3",
      agents: [{ id: "ag-first", ordinal: 1 }],
    });
    await flush();

    expect(standing).toHaveLength(1);
    expect(standing[0].agents[0].id).toBe("ag-first");
  });
});
