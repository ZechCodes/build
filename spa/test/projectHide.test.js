// Hiding one project on a machine that has gone.
//
// A project on a machine that cannot answer is nothing but cache, so hiding it
// IS dropping that cache — from the snapshot the feed is serving right now and
// from the per-device record that seeds the next boot paint. Nothing is
// remembered: there is deliberately no hidden list, because a list would keep
// the project gone after its machine came back.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

// The device's view as the feed stamps it: every row carries the account-wide
// project key, which is what a hide is by — both machines mint a `proj-1`.
const view = () => ({
  items: [
    { kind: "branch", project_id: "proj-1", projectKey: "dev-1/proj-1", run_id: "run-1", branch: "build/login" },
    { kind: "issue", project_id: "proj-2", projectKey: "dev-1/proj-2", issue_id: "issue-9" },
  ],
  plans: [],
  runs: [],
  externalWorktrees: [],
  pending: [],
  projects: [
    { id: "proj-1", projectKey: "dev-1/proj-1", deviceId: "dev-1", name: "relaydb" },
    { id: "proj-2", projectKey: "dev-1/proj-2", deviceId: "dev-1", name: "dotfiles" },
  ],
  workspaces: [
    { id: "ws-1", project_id: "proj-1", projectKey: "dev-1/proj-1", workspaceKey: "dev-1/ws-1", name: "Login" },
    { id: "ws-2", project_id: "proj-2", projectKey: "dev-1/proj-2", workspaceKey: "dev-1/ws-2", name: "Dots" },
  ],
});

const FEED = { deviceId: "dev-1", entityId: "", kind: "feed" };

let dropped;
let live;

vi.mock("../src/core/taskFeed.js", () => ({
  dropFeedProject: (deviceId, projectKey) => {
    dropped.push([deviceId, projectKey]);
    return live;
  },
}));

let cache;
let hideProject;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  dropped = [];
  live = view();
  cache = await import("../src/core/localCache.js");
  ({ hideProject } = await import("../src/core/projectHide.js"));
});

describe("hiding a project", () => {
  it("drops it from the snapshot in memory and from the record on disk", async () => {
    await cache.writeCached(FEED, view());

    await hideProject({ deviceId: "dev-1", projectKey: "dev-1/proj-1" });

    // The snapshot in memory is where the block is painted from right now: a
    // hide that only cleared the disk would repaint it on the next tick.
    expect(dropped).toEqual([["dev-1", "dev-1/proj-1"]]);

    const { value } = await cache.readCached(FEED);
    expect(value.items.map((row) => row.projectKey)).toEqual(["dev-1/proj-2"]);
    expect(value.projects.map((row) => row.id)).toEqual(["proj-2"]);
    expect(value.workspaces.map((row) => row.id)).toEqual(["ws-2"]);
  });

  // The cache is keyed by (device, entity), and a project is not an entity of
  // its own: what it holds is its rows' conversations, diffs and file trees.
  it("evicts everything cached under the project's own rows, and nothing else", async () => {
    await cache.writeCached(FEED, view());
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "thread" }, { lines: ["gone"] });
    await cache.writeCached({ deviceId: "dev-1", entityId: "ws-1", kind: "files" }, ["src/a.js"]);
    await cache.writeCached({ deviceId: "dev-1", entityId: "issue-9", kind: "thread" }, { lines: ["kept"] });
    // Another machine's `proj-1` is a different project and keeps everything.
    await cache.writeCached({ deviceId: "dev-2", entityId: "run-1", kind: "thread" }, { lines: ["theirs"] });

    await hideProject({ deviceId: "dev-1", projectKey: "dev-1/proj-1" });

    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "thread" })).toBeUndefined();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "ws-1", kind: "files" })).toBeUndefined();
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "issue-9", kind: "thread" })).value).toEqual({ lines: ["kept"] });
    expect((await cache.readCached({ deviceId: "dev-2", entityId: "run-1", kind: "thread" })).value).toEqual({ lines: ["theirs"] });
  });

  // The two layers do not have to agree: a reload leaves rows on disk the
  // snapshot has not seen, and a session leaves rows in memory nothing has
  // persisted yet. An entity missed in either keeps a conversation cached for a
  // project the reader has put away.
  it("reads the entities to evict off both layers", async () => {
    await cache.writeCached(FEED, {
      ...view(),
      items: [{ kind: "branch", project_id: "proj-1", projectKey: "dev-1/proj-1", run_id: "run-only-on-disk" }],
    });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-only-on-disk", kind: "thread" }, { lines: ["stale"] });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "thread" }, { lines: ["live"] });

    await hideProject({ deviceId: "dev-1", projectKey: "dev-1/proj-1" });

    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-only-on-disk", kind: "thread" })).toBeUndefined();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "thread" })).toBeUndefined();
  });

  it("leaves a device with nothing cached alone rather than writing an empty record", async () => {
    await hideProject({ deviceId: "dev-1", projectKey: "dev-1/proj-1" });
    expect(await cache.readCached(FEED)).toBeUndefined();
    expect(dropped).toEqual([["dev-1", "dev-1/proj-1"]]);
  });

  it("does nothing at all without both a device and a project to name", async () => {
    await cache.writeCached(FEED, view());
    await hideProject({ deviceId: "", projectKey: "dev-1/proj-1" });
    await hideProject({ deviceId: "dev-1", projectKey: "" });
    expect(dropped).toEqual([]);
    expect((await cache.readCached(FEED)).value.projects).toHaveLength(2);
  });
});
