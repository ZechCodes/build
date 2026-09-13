// @vitest-environment jsdom
// The shared feed snapshot normalizes the wire's shapes: the daemon's
// project.list rows carry `project_id`, while every consumer of the snapshot
// (the toolbar's scope, its menu, the project names) reads `id`. The feed is
// the one place the wire is read, so it is the one place the key is bridged.

import { describe, it, expect, afterEach, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let App;
let subscribeFeed, refreshFeed, resetFeedScope, startFeed, stopFeed;
let armChangeEvents, dispatchChangeEvent, resetChangeEvents, SAFETY_POLL_MS;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ App } = await import("../src/app.js"));
  ({ subscribeFeed, refreshFeed, resetFeedScope, startFeed, stopFeed } = await import("../src/core/taskFeed.js"));
  ({ armChangeEvents, dispatchChangeEvent, resetChangeEvents, SAFETY_POLL_MS } = await import(
    "../src/core/changeEvents.js"
  ));
});

afterEach(() => {
  stopFeed();
  resetChangeEvents();
  vi.useRealTimers();
});

/** The feed's reads, counted: one tick is a board.list and a project.list. */
const countingCall = () => {
  const call = vi.fn(async (method) =>
    method === "project.list" ? { projects: [] } : { items: [] },
  );
  App.call = call;
  return () => call.mock.calls.filter(([method]) => method === "board.list").length;
};

/** startFeed's own first read, awaited, so a test counts only what follows. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("the shared feed", () => {
  it("joins board work summaries onto their workspace rows", async () => {
    App.call = vi.fn(async (method) => {
      if (method === "project.list") return { projects: [] };
      if (method === "workspace.list") {
        return { workspaces: [{ id: "ws-1" }, { workspace_id: "ws-2" }, { id: "ws-3" }] };
      }
      return {
        items: [],
        workspace_summaries: [
          { workspace_id: "ws-1", work_summary: { pushes: 2, additions: 8, deletions: 3 } },
          { workspace_id: "ws-2", work_summary: null },
        ],
      };
    });
    let snapshot = null;
    subscribeFeed((feed) => {
      snapshot = feed;
    });

    await refreshFeed();

    expect(snapshot.workspaces).toEqual([
      { id: "ws-1", work_summary: { pushes: 2, additions: 8, deletions: 3 } },
      { workspace_id: "ws-2", work_summary: null },
      { id: "ws-3" },
    ]);
  });

  it("gives every project row the id consumers read, from the wire's project_id", async () => {
    App.call = vi.fn(async (method) =>
      method === "project.list"
        ? { projects: [{ project_id: "proj-1", name: "relaydb", path: "/r" }] }
        : { items: [] },
    );
    let snapshot = null;
    subscribeFeed((feed) => {
      snapshot = feed;
    });
    await refreshFeed();
    expect(snapshot.projects).toEqual([
      { id: "proj-1", project_id: "proj-1", name: "relaydb", path: "/r" },
    ]);
  });

  it("discards a late snapshot from the session that was replaced", async () => {
    const releases = [];
    App.session = { deviceId: "device-a" };
    App.call = vi.fn((method) => new Promise((resolve) => releases.push([method, resolve])));
    const seen = [];
    subscribeFeed((feed) => seen.push(feed));
    const stale = refreshFeed();

    App.session = { deviceId: "device-b" };
    App.call = vi.fn(async (method) => method === "project.list" ? { projects: [] } : { items: [] });
    for (const [method, resolve] of releases) {
      resolve(method === "project.list" ? { projects: [{ project_id: "old" }] } : { items: [{ id: "old" }] });
    }
    await stale;

    expect(seen).toEqual([]);
  });

  it("takes the previous device's snapshot out of replay during a switch", async () => {
    App.call = vi.fn(async (method) =>
      method === "project.list" ? { projects: [] } : { items: [{ id: "device-a-item" }] },
    );
    await refreshFeed();

    resetFeedScope();
    let replayed;
    subscribeFeed((feed) => { replayed = feed; });

    expect(replayed.cached).toBe(true);
    expect(replayed.items).toEqual([]);
  });
});

describe("the feed against a bridge that pushes", () => {
  it("reads once per board.changed, and no more", async () => {
    const reads = countingCall();
    armChangeEvents({ push_events: true });
    startFeed();
    await settle();
    expect(reads()).toBe(1); // startFeed's own first read

    dispatchChangeEvent({ type: "board.changed" });
    await settle();
    expect(reads()).toBe(2);
  });

  it("ignores an entity's event — the board's own event covers the feed", async () => {
    const reads = countingCall();
    armChangeEvents({ push_events: true });
    startFeed();
    await settle();
    dispatchChangeEvent({ type: "entity.changed", id: "run-7" });
    await settle();
    expect(reads()).toBe(1);
  });

  it("stands its fast poll down to the safety poll", async () => {
    vi.useFakeTimers();
    const reads = countingCall();
    armChangeEvents({ push_events: true });
    startFeed(2000);
    await vi.advanceTimersByTimeAsync(SAFETY_POLL_MS - 1000);
    expect(reads()).toBe(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(reads()).toBe(2);
  });
});

describe("the feed against a bridge that does not", () => {
  it("keeps polling at its own cadence", async () => {
    vi.useFakeTimers();
    const reads = countingCall();
    startFeed(2000);
    await vi.advanceTimersByTimeAsync(2000);
    expect(reads()).toBe(2);
    await vi.advanceTimersByTimeAsync(2000);
    expect(reads()).toBe(3);
  });

  it("does not read on a change event it was never told to expect", async () => {
    const reads = countingCall();
    startFeed();
    await settle();
    dispatchChangeEvent({ type: "board.changed" });
    await settle();
    expect(reads()).toBe(1);
  });
});

// ---- the cached boot paint -----------------------------------------------------
// The last snapshot the syncer persisted paints the inbox before the bridge
// answers. It is marked `cached: true` so the sync layer does not treat its own
// echo as news, and a live answer always wins the race.
describe("the feed's cached boot paint", () => {
  it("delivers the cached snapshot while the bridge is still being asked", async () => {
    App.selectedDeviceId = "dev-1";
    const { writeCached } = await import("../src/core/localCache.js");
    await writeCached(
      { deviceId: "dev-1", entityId: "", kind: "feed" },
      { items: [{ kind: "branch", branch: "build/x" }], plans: [], runs: [], externalWorktrees: [], projects: [], primaryChanges: [] },
    );
    App.call = vi.fn(() => new Promise(() => {})); // the bridge never answers
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));
    startFeed();
    for (let i = 0; i < 15; i++) await settle();
    expect(seen).toHaveLength(1);
    expect(seen[0].cached).toBe(true);
    expect(seen[0].items).toHaveLength(1);
  });

  // What a verb was doing last time this browser was open is not a fact about
  // now: those verbs settled long ago, and the live answer names whatever is
  // running today.
  it("paints nothing in flight from the cache", async () => {
    App.selectedDeviceId = "dev-1";
    const { writeCached } = await import("../src/core/localCache.js");
    await writeCached(
      { deviceId: "dev-1", entityId: "", kind: "feed" },
      {
        items: [],
        plans: [],
        runs: [],
        externalWorktrees: [],
        pending: [{ entity_id: "wt-old", project_id: "p1", title: "gone", state: "creating" }],
        projects: [],
        primaryChanges: [],
      },
    );
    App.call = vi.fn(() => new Promise(() => {}));
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));
    startFeed();
    for (let i = 0; i < 15; i++) await settle();
    expect(seen[0].cached).toBe(true);
    expect(seen[0].pending).toEqual([]);
  });

  it("never paints the cache over a live answer", async () => {
    App.selectedDeviceId = "dev-1";
    const { writeCached } = await import("../src/core/localCache.js");
    await writeCached(
      { deviceId: "dev-1", entityId: "", kind: "feed" },
      { items: [{ kind: "branch", branch: "stale" }], plans: [], runs: [], externalWorktrees: [], projects: [], primaryChanges: [] },
    );
    App.call = vi.fn(async (method) => (method === "project.list" ? { projects: [] } : { items: [] }));
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));
    startFeed();
    for (let i = 0; i < 15; i++) await settle();
    // A cached paint may land first (the mirror answers in a microtask); what
    // must hold is that the live answer ends the sequence and nothing cached
    // ever paints after it.
    const lastLive = seen.map((snapshot) => !snapshot.cached).lastIndexOf(true);
    expect(lastLive).toBe(seen.length - 1);
    expect(seen[seen.length - 1].items).toEqual([]);
  });

  it("paints nothing from the cache when no device was ever chosen", async () => {
    App.selectedDeviceId = null;
    App.session = null;
    App.call = vi.fn(() => new Promise(() => {}));
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));
    startFeed();
    for (let i = 0; i < 15; i++) await settle();
    expect(seen).toHaveLength(0);
  });
});
