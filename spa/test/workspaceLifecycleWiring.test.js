// @vitest-environment jsdom
// A workspace's lifecycle verdict (#135), from the wire to the row, with every
// layer in between real: the sync pass reads `workspace.list` and writes the
// cache, the feed reads the cache, and the project page reads the feed. Only
// the edges are stood in: the device's bridge, the device registry, and the
// greeting's capabilities.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/app.js", () => ({ App }));

vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "issues"] } }),
  subscriptionsSettledFor: async () => {},
  onSubscriptionHeld: () => () => {},
  watchChanges: () => ({ dispose: () => {} }),
}));

const contexts = new Map();
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: (deviceId) => contexts.get(deviceId) || null,
  liveContexts: () => [...contexts.values()],
  onDeviceStateChanged: () => () => {},
}));

const verdict = {
  measured_at_ms: 1_790_000_000_000,
  last_activity_ms: 1_789_900_000_000,
  idle: true,
  reclaimable: false,
  holds: ["dirty", "terminal_open"],
  issues: [{ issue_id: "issue-1", number: 135, title: "Lifecycle", status: "done", state: "open" }],
  dirty_files: 3,
  unpushed_commits: 0,
  behind_commits: 0,
  size_bytes: 17_200_000_000,
  pruned_bytes: 0,
  pruned_at_ms: null,
  noticed_at_ms: 1_790_000_000_000,
};

const ANSWERS = {
  "board.list": () => ({ items: [] }),
  "project.list": () => ({ projects: [{ project_id: "p1", name: "build" }] }),
  "workspace.list": () => ({
    workspaces: [{
      id: "ws-1",
      workspace_id: "ws-1",
      project_id: "p1",
      name: "lifecycle",
      status: "ready",
      directories: [{ source_id: "repo", branch: "build/lifecycle", is_git: true }],
      lifecycle: verdict,
    }],
  }),
};

const call = vi.fn(async (method) => (ANSWERS[method] || (() => ({})))());

let sync;
let feed;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  contexts.clear();
  contexts.set("dev-1", {
    deviceId: "dev-1",
    rpc: call,
    session: { device: "dev-1" },
    greeted: Promise.resolve(),
    cacheScope: { deviceId: "dev-1", active: () => true },
    active: () => true,
  });
  sync = await import("../src/core/cacheSync.js");
  feed = await import("../src/core/taskFeed.js");
});

afterEach(() => {
  sync.stopCacheSync();
  feed.stopFeed();
});

/** The first feed snapshot that has the workspace in it. */
const snapshotWithWorkspace = () =>
  new Promise((resolve) => {
    const stop = feed.subscribeFeed((snapshot) => {
      if (!snapshot.workspaces?.some((workspace) => workspace.workspace_id === "ws-1")) return;
      queueMicrotask(() => stop());
      resolve(snapshot);
    });
  });

describe("a workspace's lifecycle, wire to row", () => {
  it("rides workspace.list through the cache and the feed onto the project page's row", async () => {
    await feed.startFeed();
    const arrived = snapshotWithWorkspace();
    sync.startCacheSync();

    const snapshot = await arrived;

    const row = snapshot.workspaces.find((workspace) => workspace.workspace_id === "ws-1");
    expect(row.lifecycle).toEqual(verdict);
    const { projectPageModel } = await import("../src/core/projectPageModel.js");
    const page = projectPageModel(snapshot, { name: "project", deviceId: "dev-1", projectId: "p1" });
    const listed = page.rows.find((each) => each.workspaceKey === row.workspaceKey);
    expect(listed.lifecycle).toEqual({
      reclaimable: false,
      text: "Idle · 3 uncommitted files, a terminal open · 17.2 GB",
    });
  });
});
