// @vitest-environment jsdom
// The shared feed: a cache view, one snapshot out.
//
// Nothing here reads a bridge. Each device's rows, projects and workspaces are
// read off the cache and re-read when the cache announces that one of them
// moved; the merge is what subscribers get, with every device's own view beside
// it under `devices`. Asking for a refresh asks the sync layer for a pass — the
// one reader of the wire — and the pass's writes come back as announcements.

import { describe, it, expect, afterEach, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { projectAgentEntries } from "../src/core/inboxProjectAgent.js";
import { watchedWorkspaceEntries } from "../src/core/inbox.js";

const syncDevice = vi.fn(async () => true);
vi.mock("../src/core/cacheSync.js", () => ({ syncDevice: (deviceId) => syncDevice(deviceId) }));

let App;
let subscribeFeed, refreshFeed, startFeed, stopFeed, joinFeed, dropFeedDevice, dropFeedProject;
let adoptBridgeSelection, adoptDeviceSession, retireDeviceContext, resetDeviceContexts;
let writeCached, DEVICES_ADDRESS;

beforeEach(async () => {
  vi.resetModules();
  syncDevice.mockClear();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ App } = await import("../src/app.js"));
  ({ subscribeFeed, refreshFeed, startFeed, stopFeed, joinFeed, dropFeedDevice, dropFeedProject } = await import(
    "../src/core/taskFeed.js"
  ));
  ({ adoptBridgeSelection, adoptDeviceSession, retireDeviceContext, resetDeviceContexts } = await import(
    "../src/core/deviceContexts.js"
  ));
  ({ writeCached, DEVICES_ADDRESS } = await import("../src/core/localCache.js"));
  App.devices = [];
});

afterEach(() => {
  stopFeed();
  resetDeviceContexts();
  App.devices = [];
  vi.useRealTimers();
});

/** A device the account knows and a live session on it whose bridge has
 *  greeted. The feed asks it nothing; a refresh asks the sync layer, which is
 *  what the call counter below stands for. */
function device(deviceId, call = vi.fn(async () => ({}))) {
  App.devices = [...App.devices, { id: deviceId, name: deviceId, status: "online" }];
  const context = adoptDeviceSession({ deviceId, call, close: () => {}, peer: () => {}, onCarrier: () => {} });
  adoptBridgeSelection(context, { major: 2, version: "2.0.0" }, {});
  return context;
}

/** The records one pass writes for a device: the feed's own collections, the
 *  two lists, and a row per work item. */
async function writeDevice(deviceId, { items = [], projects = [], workspaces = [], ...rest } = {}) {
  await writeCached(
    { deviceId, entityId: "", kind: "feed" },
    { items, plans: [], runs: [], externalWorktrees: [], pending: [], projects, workspaces, ...rest },
  );
  await writeCached({ deviceId, entityId: "", kind: "projects" }, projects);
  await writeCached({ deviceId, entityId: "", kind: "workspaces" }, workspaces);
  for (const item of items) await writeRow(deviceId, item);
}

const rowId = (item) => item.entity_id || item.run_id || item.task_id || item.worktree_id || item.id;

const writeRow = (deviceId, item) => writeCached({ deviceId, entityId: rowId(item), kind: "row" }, item);

const branchRow = (over = {}) => ({
  kind: "branch",
  project_id: "proj-1",
  branch: "build/x",
  entity_id: "run-1",
  deviceId: "dev-1",
  projectKey: "dev-1/proj-1",
  ...over,
});

/** Long enough for a read and the announcement behind it to settle. */
const settle = async () => {
  for (let index = 0; index < 20; index += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

describe("the feed's cached running-agent rollups (#386)", () => {
  const parentRow = (agents, over = {}) => branchRow({ entity_id: "run-project", agents, ...over });
  const project = { id: "proj-1", name: "Build", entity_id: "run-project" };
  const agentOf = (snapshot, deviceId = "dev-1") => snapshot.devices[deviceId].items
    .find((row) => row.entity_id === "run-project").agents[0];
  const support = (deviceId, namesMakers) => writeCached({ deviceId, entityId: "", kind: "agent-lineage-support" }, { namesMakers });

  it("pulses project and workspace rows from raw harness descendants, then stops on a cache write", async () => {
    const waiting = { id: "parent", watched: true, working: false,
      surfaces: { subagents: [{ id: "harness-child", state: "running", started_at: 100 }] } };
    const workspace = { id: "ws-1", project_id: "proj-1", name: "Checkout", status: "ready", entity_id: "run-workspace" };
    await writeDevice("dev-1", { projects: [project], workspaces: [workspace], items: [parentRow([waiting]),
      branchRow({ entity_id: "run-workspace", agents: [{ ...waiting, id: "workspace-parent" }] })] });
    await writeCached(DEVICES_ADDRESS, [{ id: "dev-1" }]);
    let snapshot;
    subscribeFeed((next) => { snapshot = next; });
    await startFeed();
    const view = snapshot.devices["dev-1"];
    expect(agentOf(snapshot)).toMatchObject({ working: false, agents_running: 1 });
    expect(projectAgentEntries(view.projects, view.items, view.runs)[0]).toMatchObject({ working: true, watchedWorking: true });
    expect(watchedWorkspaceEntries(view.workspaces, view.projects, view.items, view.runs)[0])
      .toMatchObject({ working: true, watchedWorking: true, runningCount: 1 });
    await writeRow("dev-1", parentRow([{ ...waiting, surfaces: { subagents: [{ state: "idle" }] } }]));
    await vi.waitFor(() => expect(projectAgentEntries(snapshot.projects, snapshot.items, snapshot.runs)[0].working).toBe(false));
    expect(agentOf(snapshot).working).toBe(false);
    expect(syncDevice).not.toHaveBeenCalled();
  });

  it("rolls up hidden Build descendants transitively with cached capability gating and child-stop repaint", async () => {
    const parent = parentRow([{ id: "parent", watched: true, working: false }]);
    const child = branchRow({ entity_id: "run-child", agents: [{ id: "child", created_by: "parent", working: false }] });
    const grandchild = branchRow({ entity_id: "run-grandchild", agents: [{ id: "grandchild", created_by: "child", working: true }] });
    await writeDevice("dev-1", { projects: [project], items: [parent], runs: [parent, child, grandchild] });
    await support("dev-1", true);
    await writeCached(DEVICES_ADDRESS, [{ id: "dev-1" }]);
    let snapshot;
    subscribeFeed((next) => { snapshot = next; });
    await startFeed();
    expect(agentOf(snapshot)).toMatchObject({ working: false, agents_running: 1 });
    await support("dev-1", false);
    await vi.waitFor(() => expect(projectAgentEntries(snapshot.projects, snapshot.items, snapshot.runs)[0].working).toBe(false));
    await support("dev-1", true);
    await vi.waitFor(() => expect(agentOf(snapshot).agents_running).toBe(1));
    await writeRow("dev-1", { ...grandchild, agents: [{ ...grandchild.agents[0], working: false }] });
    await vi.waitFor(() => expect(projectAgentEntries(snapshot.projects, snapshot.items, snapshot.runs)[0].working).toBe(false));
    expect(syncDevice).not.toHaveBeenCalled();
  });

  it("uses the newer stopped roster over an older running board copy", async () => {
    const older = parentRow([{ id: "parent", watched: true, working: false, surfaces: { subagents: [{ state: "running" }] } }]);
    const newer = parentRow([{ id: "parent", watched: true, working: false, surfaces: { subagents: [] } }]);
    await writeDevice("dev-1", { projects: [project], items: [older], runs: [older] });
    await writeRow("dev-1", newer);
    await writeCached(DEVICES_ADDRESS, [{ id: "dev-1" }]);
    let snapshot;
    subscribeFeed((next) => { snapshot = next; });
    await startFeed();
    expect(projectAgentEntries(snapshot.projects, snapshot.items, snapshot.runs)[0].working).toBe(false);
    expect(agentOf(snapshot).agents_running || 0).toBe(0);
  });

  it("keeps identical agent ids on other projects and devices out of a parent's lineage", async () => {
    const parent = parentRow([{ id: "parent", watched: true, working: false }]);
    const unrelated = branchRow({ project_id: "proj-2", projectKey: "dev-1/proj-2", entity_id: "other-project",
      agents: [{ id: "child", created_by: "parent", working: true }] });
    await writeDevice("dev-1", { projects: [project], items: [parent, unrelated] });
    await writeDevice("dev-2", { projects: [project], items: [parentRow([{ id: "parent", watched: true, working: true }], { deviceId: "dev-2" })] });
    await support("dev-1", true);
    await support("dev-2", true);
    await writeCached(DEVICES_ADDRESS, [{ id: "dev-1" }, { id: "dev-2" }]);
    let snapshot;
    subscribeFeed((next) => { snapshot = next; });
    await startFeed();
    expect(agentOf(snapshot)).toMatchObject({ working: false });
    expect(agentOf(snapshot).agents_running || 0).toBe(0);
    expect(agentOf(snapshot, "dev-2").working).toBe(true);
  });
});

describe("the feed as a cache view", () => {
  it("delivers every known device's cached rows with nothing asked of any bridge", async () => {
    await writeDevice("dev-a", { items: [branchRow({ entity_id: "run-a", deviceId: "dev-a" })] });
    await writeDevice("dev-b", { items: [branchRow({ entity_id: "run-b", deviceId: "dev-b" })] });
    const call = vi.fn(async () => ({}));
    device("dev-a", call);
    device("dev-b", call);
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));

    await startFeed();
    await settle();

    const last = seen[seen.length - 1];
    expect(last.items.map((item) => item.entity_id).sort()).toEqual(["run-a", "run-b"]);
    expect(call).not.toHaveBeenCalled();
    expect(syncDevice).not.toHaveBeenCalled();
  });

  it("delivers on a feed announcement, and asks no bridge for the board", async () => {
    const call = vi.fn(async () => ({}));
    device("dev-1", call);
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));
    await startFeed();
    await settle();

    await writeDevice("dev-1", { items: [branchRow()], projects: [{ id: "proj-1", deviceId: "dev-1" }] });
    await settle();

    const last = seen[seen.length - 1];
    expect(last.items.map((item) => item.branch)).toEqual(["build/x"]);
    expect(last.projects.map((project) => project.id)).toEqual(["proj-1"]);
    expect(call.mock.calls.filter(([method]) => method === "board.list")).toHaveLength(0);
  });

  it("takes a row that rode in on its own push, with no pass behind it", async () => {
    await writeDevice("dev-1", { items: [] });
    device("dev-1");
    let snapshot = null;
    subscribeFeed((next) => (snapshot = next));
    await startFeed();
    await settle();

    await writeRow("dev-1", branchRow({ entity_id: "run-new", branch: "build/new" }));
    await settle();

    expect(snapshot.items.map((item) => item.branch)).toEqual(["build/new"]);
  });

  it("takes a row's own record over the board list it was last read with", async () => {
    await writeDevice("dev-1", { items: [branchRow({ state: "building" })] });
    device("dev-1");
    let snapshot = null;
    subscribeFeed((next) => (snapshot = next));
    await startFeed();
    await settle();
    expect(snapshot.items.map((item) => item.state)).toEqual(["building"]);

    await writeRow("dev-1", branchRow({ state: "review" }));
    await settle();

    expect(snapshot.items.map((item) => item.state)).toEqual(["review"]);
  });

  // A bare checkout nobody has claimed holds no conversation and so has no
  // entity to be addressed by. It is on the board and nowhere else, and the
  // rail lists it all the same.
  it("keeps a board row that names no entity of its own", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "", kind: "feed" },
      { items: [{ kind: "branch", project_id: "proj-1", branch: "build/loose", deviceId: "dev-1" }] },
    );
    device("dev-1");
    let snapshot = null;
    subscribeFeed((next) => (snapshot = next));
    await startFeed();
    await settle();

    expect(snapshot.items.map((item) => item.branch)).toEqual(["build/loose"]);
  });

  it("runs on no timer at all", async () => {
    vi.useFakeTimers();
    const call = vi.fn(async () => ({}));
    device("dev-1", call);
    subscribeFeed(() => {});
    startFeed();
    await vi.advanceTimersByTimeAsync(60 * 60 * 1000);

    expect(call).not.toHaveBeenCalled();
    expect(syncDevice).not.toHaveBeenCalled();
  });

  it("seeds from the cached device list before the account list has answered", async () => {
    await writeCached(DEVICES_ADDRESS, [{ id: "dev-a" }, { id: "dev-b" }]);
    await writeDevice("dev-a", { items: [branchRow({ entity_id: "run-a", branch: "a", deviceId: "dev-a" })] });
    await writeDevice("dev-b", { items: [branchRow({ entity_id: "run-b", branch: "b", deviceId: "dev-b" })] });
    App.devices = [];
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));

    await startFeed();
    await settle();

    const last = seen[seen.length - 1];
    expect(last.items.map((item) => item.branch).sort()).toEqual(["a", "b"]);
  });

  it("holds each device's own view under devices[id]", async () => {
    await writeDevice("dev-a", { items: [] });
    await writeDevice("dev-b", { items: [branchRow({ entity_id: "run-b", deviceId: "dev-b" })] });
    device("dev-a");
    device("dev-b");
    let snapshot = null;
    subscribeFeed((next) => (snapshot = next));
    await startFeed();
    await settle();

    expect(snapshot.devices["dev-a"].items).toEqual([]);
    expect(snapshot.devices["dev-b"].items).toHaveLength(1);
  });

  it("paints nothing for a device the cache holds nothing for", async () => {
    device("dev-1");
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));
    await startFeed();
    await settle();

    expect(seen).toEqual([]);
  });

  it("paints nothing in flight from the boot read, and takes it from a live write", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "", kind: "feed" },
      { items: [], pending: [{ entity_id: "wt-old", title: "gone", state: "creating" }] },
    );
    device("dev-1");
    let snapshot = null;
    subscribeFeed((next) => (snapshot = next));
    await startFeed();
    await settle();
    expect(snapshot.pending).toEqual([]);
    expect(snapshot.cached).toBe(true);

    await writeCached(
      { deviceId: "dev-1", entityId: "", kind: "feed" },
      { items: [], pending: [{ entity_id: "wt-new", title: "making", state: "creating" }] },
    );
    await settle();

    expect(snapshot.pending.map((row) => row.entity_id)).toEqual(["wt-new"]);
    expect(snapshot.cached).toBe(false);
  });
});

describe("asking the feed to refresh", () => {
  it("asks the sync layer for a pass rather than reading the board itself", async () => {
    const call = vi.fn(async () => ({}));
    device("dev-1", call);
    await startFeed();
    await settle();

    await refreshFeed();

    expect(syncDevice).toHaveBeenCalledWith("dev-1");
    expect(call).not.toHaveBeenCalled();
  });

  it("asks for one named device's pass and no other's", async () => {
    device("dev-a");
    device("dev-b");
    await startFeed();
    await settle();

    await refreshFeed("dev-b");

    expect(syncDevice.mock.calls).toEqual([["dev-b"]]);
  });
});

describe("devices coming and going", () => {
  it("watches a device that joins after the feed started", async () => {
    device("dev-a");
    await startFeed();
    await settle();
    const contextB = device("dev-b");
    await writeDevice("dev-b", { items: [branchRow({ entity_id: "run-b", deviceId: "dev-b" })] });

    joinFeed(contextB);
    await settle();

    let snapshot = null;
    subscribeFeed((next) => (snapshot = next));
    expect(snapshot.items.map((item) => item.entity_id)).toEqual(["run-b"]);
  });

  it("takes a retired device's rows out of the merge and delivers what is left", async () => {
    await writeDevice("dev-a", { items: [branchRow({ entity_id: "run-a", deviceId: "dev-a" })] });
    await writeDevice("dev-b", { items: [branchRow({ entity_id: "run-b", deviceId: "dev-b" })] });
    device("dev-a");
    device("dev-b");
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));
    await startFeed();
    await settle();

    dropFeedDevice("dev-a");

    const last = seen[seen.length - 1];
    expect(last.items.map((item) => item.entity_id)).toEqual(["run-b"]);
    expect(last.devices["dev-a"]).toBeUndefined();
  });

  it("stops hearing the cache for a device it dropped", async () => {
    await writeDevice("dev-1", { items: [branchRow()] });
    device("dev-1");
    let snapshot = null;
    subscribeFeed((next) => (snapshot = next));
    await startFeed();
    await settle();

    dropFeedDevice("dev-1");
    await writeRow("dev-1", branchRow({ entity_id: "run-2" }));
    await settle();

    expect(snapshot.devices["dev-1"]).toBeUndefined();
  });

  it("drops a retired context's device without waiting for a read", async () => {
    await writeDevice("dev-a", { items: [branchRow({ entity_id: "run-a", deviceId: "dev-a" })] });
    device("dev-a");
    let snapshot = null;
    subscribeFeed((next) => (snapshot = next));
    await startFeed();
    await settle();

    retireDeviceContext("dev-a");
    await settle();

    expect(snapshot.devices["dev-a"]).toBeUndefined();
  });
});

describe("hiding a project", () => {
  it("takes that project's rows out of the device's view at once", async () => {
    await writeDevice("dev-1", {
      items: [branchRow(), branchRow({ entity_id: "run-2", project_id: "proj-2", projectKey: "dev-1/proj-2" })],
    });
    device("dev-1");
    let snapshot = null;
    subscribeFeed((next) => (snapshot = next));
    await startFeed();
    await settle();

    const view = dropFeedProject("dev-1", "dev-1/proj-1");

    expect(view.items).toHaveLength(2); // the view as it stood, for the caller to evict off
    expect(snapshot.items.map((item) => item.entity_id)).toEqual(["run-2"]);
  });
});
