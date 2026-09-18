// @vitest-environment jsdom
// The shared feed: one poller per device, one snapshot out.
//
// Each device's snapshot normalizes that device's wire (the daemon's
// project.list rows carry `project_id`, while every consumer reads `id`) and is
// stamped with the device that answered — two machines both call their first
// project `proj-1`. The merge is what subscribers get, with every device's own
// view beside it under `devices`.

import { describe, it, expect, afterEach, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let App;
let subscribeFeed, refreshFeed, startFeed, stopFeed, joinFeed, dropFeedDevice;
let adoptBridgeSelection, adoptDeviceSession, retireDeviceContext, resetDeviceContexts;
let armChangeEvents, dispatchChangeEvent, resetChangeEvents, SAFETY_POLL_MS;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ App } = await import("../src/app.js"));
  ({ subscribeFeed, refreshFeed, startFeed, stopFeed, joinFeed, dropFeedDevice } = await import(
    "../src/core/taskFeed.js"
  ));
  ({ adoptBridgeSelection, adoptDeviceSession, retireDeviceContext, resetDeviceContexts } = await import(
    "../src/core/deviceContexts.js"
  ));
  ({ armChangeEvents, dispatchChangeEvent, resetChangeEvents, SAFETY_POLL_MS } = await import(
    "../src/core/changeEvents.js"
  ));
  App.devices = [];
});

afterEach(() => {
  stopFeed();
  resetChangeEvents();
  resetDeviceContexts();
  App.devices = [];
  vi.useRealTimers();
});

/** A device the account knows and a live session on it whose bridge has
 *  greeted, which is what the feed polls: contexts, not the App's fields. The
 *  greeting is what says which API major the bridge speaks, and connection.js
 *  settles it for every machine it lands — the feed reads none before it has. */
function device(deviceId, call) {
  App.devices = [...App.devices, { id: deviceId, name: deviceId, status: "online" }];
  const context = adoptDeviceSession({ deviceId, call, close: () => {}, peer: () => {}, onCarrier: () => {} });
  adoptBridgeSelection(context, { major: 1, version: "1.0.0" }, {});
  return context;
}

/** An empty board and project list, answered by whichever device asks. */
const emptyCall = () =>
  vi.fn(async (method) => (method === "project.list" ? { projects: [] } : { items: [] }));

/** The feed's reads, counted: one tick is a board.list and a project.list. */
const countingCall = (deviceId = "dev-1") => {
  const call = emptyCall();
  device(deviceId, call);
  return () => call.mock.calls.filter(([method]) => method === "board.list").length;
};

/** startFeed's own first read, awaited, so a test counts only what follows. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("the shared feed", () => {
  it("joins board work summaries onto their workspace rows, stamped with the device that answered", async () => {
    device(
      "dev-1",
      vi.fn(async (method) => {
        if (method === "project.list") return { projects: [] };
        if (method === "workspace.list") {
          return {
            workspaces: [
              { id: "ws-1", project_id: "proj-1" },
              { workspace_id: "ws-2", project_id: "proj-1" },
              { id: "ws-3", project_id: "proj-2" },
            ],
          };
        }
        return {
          items: [],
          workspace_summaries: [
            {
              workspace_id: "ws-1",
              work_summary: { pushes: 2, additions: 8, deletions: 3 },
              can_finish: false,
              finish_blockers: ["unpushed"],
            },
            { workspace_id: "ws-2", work_summary: null, can_finish: true, finish_blockers: [] },
          ],
        };
      }),
    );
    let snapshot = null;
    subscribeFeed((feed) => {
      snapshot = feed;
    });

    await refreshFeed();

    expect(snapshot.workspaces).toEqual([
      {
        id: "ws-1",
        project_id: "proj-1",
        deviceId: "dev-1",
        projectKey: "dev-1/proj-1",
        workspaceKey: "dev-1/ws-1",
        work_summary: { pushes: 2, additions: 8, deletions: 3 },
        can_finish: false,
        finish_blockers: ["unpushed"],
      },
      {
        id: "ws-2",
        workspace_id: "ws-2",
        project_id: "proj-1",
        deviceId: "dev-1",
        projectKey: "dev-1/proj-1",
        workspaceKey: "dev-1/ws-2",
        work_summary: null,
        can_finish: true,
        finish_blockers: [],
      },
      {
        id: "ws-3",
        project_id: "proj-2",
        deviceId: "dev-1",
        projectKey: "dev-1/proj-2",
        workspaceKey: "dev-1/ws-3",
      },
    ]);
  });

  it("gives every project row the id consumers read, from the wire's project_id", async () => {
    device(
      "dev-1",
      vi.fn(async (method) =>
        method === "project.list"
          ? { projects: [{ project_id: "proj-1", name: "relaydb", path: "/r" }] }
          : { items: [] },
      ),
    );
    let snapshot = null;
    subscribeFeed((feed) => {
      snapshot = feed;
    });
    await refreshFeed();
    expect(snapshot.projects).toEqual([
      {
        id: "proj-1",
        project_id: "proj-1",
        name: "relaydb",
        path: "/r",
        deviceId: "dev-1",
        projectKey: "dev-1/proj-1",
      },
    ]);
  });

  it("merges two devices' answers, stamped with their ids in App.devices order", async () => {
    const boardOf = (deviceId) =>
      vi.fn(async (method) =>
        method === "project.list"
          ? { projects: [{ project_id: "proj-1", name: deviceId }] }
          : { items: [{ kind: "branch", project_id: "proj-1", branch: deviceId }] },
      );
    device("dev-a", boardOf("dev-a"));
    device("dev-b", boardOf("dev-b"));
    let snapshot = null;
    subscribeFeed((feed) => {
      snapshot = feed;
    });
    await refreshFeed();

    expect(snapshot.items.map((item) => item.deviceId)).toEqual(["dev-a", "dev-b"]);
    expect(snapshot.projects.map((project) => project.projectKey)).toEqual(["dev-a/proj-1", "dev-b/proj-1"]);
  });

  it("holds each device's own view under devices[id]", async () => {
    device("dev-a", emptyCall());
    device(
      "dev-b",
      vi.fn(async (method) => (method === "project.list" ? { projects: [] } : { items: [{ id: "b-only" }] })),
    );
    let snapshot = null;
    subscribeFeed((feed) => {
      snapshot = feed;
    });
    await refreshFeed();

    expect(snapshot.devices["dev-a"].items).toEqual([]);
    expect(snapshot.devices["dev-b"].items).toHaveLength(1);
  });

  it("drops a late answer from a retired context", async () => {
    const releases = [];
    device("dev-a", vi.fn((method) => new Promise((resolve) => releases.push([method, resolve]))));
    const seen = [];
    subscribeFeed((feed) => seen.push(feed));
    const stale = refreshFeed();

    retireDeviceContext("dev-a");
    for (const [method, resolve] of releases) {
      resolve(method === "project.list" ? { projects: [{ project_id: "old" }] } : { items: [{ id: "old" }] });
    }
    await stale;

    expect(seen).toEqual([]);
  });

  // A bridge speaking an API major no adapter here claims is answering, in a
  // shape this tab cannot read: every answer off it would be a guess. The rows
  // it gave while it was readable stay in the merge — the rail greys them —
  // and nothing asks it for more.
  it("stops reading a device whose bridge speaks an API this app cannot read", async () => {
    const call = vi.fn(async (method) => (method === "project.list" ? { projects: [] } : { items: [{ id: "a" }] }));
    const context = device("dev-a", call);
    const reads = () => call.mock.calls.filter(([method]) => method === "board.list").length;
    let snapshot = null;
    subscribeFeed((feed) => (snapshot = feed));
    startFeed();
    await settle();
    expect(reads()).toBe(1);

    adoptBridgeSelection(context, { version: "2.0.0", unsupported: "app" }, null);
    await refreshFeed();
    await refreshFeed("dev-a");

    expect(reads()).toBe(1);
    expect(snapshot.items.map((item) => item.id)).toEqual(["a"]);
  });

  it("takes a retired device's rows out of the merge and delivers what is left", async () => {
    device("dev-a", vi.fn(async (method) => (method === "project.list" ? { projects: [] } : { items: [{ id: "a" }] })));
    device("dev-b", vi.fn(async (method) => (method === "project.list" ? { projects: [] } : { items: [{ id: "b" }] })));
    const seen = [];
    subscribeFeed((feed) => seen.push(feed));
    await refreshFeed();

    dropFeedDevice("dev-a");

    const last = seen[seen.length - 1];
    expect(last.items.map((item) => item.id)).toEqual(["b"]);
    expect(last.devices["dev-a"]).toBeUndefined();
  });
});

describe("a device that joins after the feed started", () => {
  it("gets its own board watcher and reads at once", async () => {
    armChangeEvents({ push_events: true }, "dev-a");
    armChangeEvents({ push_events: true }, "dev-b");
    const callA = emptyCall();
    device("dev-a", callA);
    const readsA = () => callA.mock.calls.filter(([method]) => method === "board.list").length;
    startFeed();
    await settle();

    const callB = emptyCall();
    const contextB = device("dev-b", callB);
    const readsB = () => callB.mock.calls.filter(([method]) => method === "board.list").length;
    joinFeed(contextB);
    await settle();
    expect(readsB()).toBe(1); // the join reads straight away

    dispatchChangeEvent({ type: "board.changed" }, "dev-b");
    await settle();
    expect(readsB()).toBe(2);
    expect(readsA()).toBe(1); // B's event is not A's
  });

  it("joins a device only once, however often it is offered", async () => {
    const call = emptyCall();
    const context = device("dev-a", call);
    const reads = () => call.mock.calls.filter(([method]) => method === "board.list").length;
    startFeed();
    await settle();
    joinFeed(context);
    joinFeed(context);
    await settle();
    expect(reads()).toBe(1);
  });
});

describe("the feed against a bridge that pushes", () => {
  it("reads once per board.changed, and no more", async () => {
    const reads = countingCall();
    armChangeEvents({ push_events: true }, "dev-1");
    startFeed();
    await settle();
    expect(reads()).toBe(1); // startFeed's own first read

    dispatchChangeEvent({ type: "board.changed" }, "dev-1");
    await settle();
    expect(reads()).toBe(2);
  });

  it("ignores an entity's event — the board's own event covers the feed", async () => {
    const reads = countingCall();
    armChangeEvents({ push_events: true }, "dev-1");
    startFeed();
    await settle();
    dispatchChangeEvent({ type: "entity.changed", id: "run-7" }, "dev-1");
    await settle();
    expect(reads()).toBe(1);
  });

  it("stands its fast poll down to the safety poll", async () => {
    vi.useFakeTimers();
    const reads = countingCall();
    armChangeEvents({ push_events: true }, "dev-1");
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
    dispatchChangeEvent({ type: "board.changed" }, "dev-1");
    await settle();
    expect(reads()).toBe(1);
  });
});

// ---- the cached boot paint -----------------------------------------------------
// The last snapshot the syncer persisted paints the inbox before the bridges
// answer — every device the account knows, not just the one creation goes to.
// It is marked `cached: true` so the sync layer does not treat its own echo as
// news, and a live answer always wins the race.
describe("the feed's cached boot paint", () => {
  const writeFeed = async (deviceId, value) => {
    const { writeCached } = await import("../src/core/localCache.js");
    await writeCached({ deviceId, entityId: "", kind: "feed" }, value);
  };

  const cachedFeed = (over = {}) => ({
    items: [],
    plans: [],
    runs: [],
    externalWorktrees: [],
    pending: [],
    projects: [],
    ...over,
  });

  it("delivers the cached snapshot while the bridge is still being asked", async () => {
    await writeFeed("dev-1", cachedFeed({ items: [{ kind: "branch", branch: "build/x" }] }));
    device("dev-1", vi.fn(() => new Promise(() => {}))); // the bridge never answers
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));
    startFeed();
    for (let i = 0; i < 15; i++) await settle();
    expect(seen).toHaveLength(1);
    expect(seen[0].cached).toBe(true);
    expect(seen[0].items).toHaveLength(1);
  });

  it("paints every device the account knows, not only the home one", async () => {
    await writeFeed("dev-a", cachedFeed({ items: [{ kind: "branch", branch: "a" }] }));
    await writeFeed("dev-b", cachedFeed({ items: [{ kind: "branch", branch: "b" }] }));
    device("dev-a", vi.fn(() => new Promise(() => {})));
    device("dev-b", vi.fn(() => new Promise(() => {})));
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));
    startFeed();
    for (let i = 0; i < 15; i++) await settle();
    const last = seen[seen.length - 1];
    expect(last.cached).toBe(true);
    expect(last.items.map((item) => item.branch)).toEqual(["a", "b"]);
  });

  // What a verb was doing last time this browser was open is not a fact about
  // now: those verbs settled long ago, and the live answer names whatever is
  // running today.
  it("paints nothing in flight from the cache", async () => {
    await writeFeed(
      "dev-1",
      cachedFeed({ pending: [{ entity_id: "wt-old", project_id: "p1", title: "gone", state: "creating" }] }),
    );
    device("dev-1", vi.fn(() => new Promise(() => {})));
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));
    startFeed();
    for (let i = 0; i < 15; i++) await settle();
    expect(seen[0].cached).toBe(true);
    expect(seen[0].pending).toEqual([]);
  });

  it("never paints the cache over a live answer", async () => {
    await writeFeed("dev-1", cachedFeed({ items: [{ kind: "branch", branch: "stale" }] }));
    device("dev-1", emptyCall());
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

  it("paints nothing from the cache when the account knows no device", async () => {
    App.devices = [];
    const seen = [];
    subscribeFeed((snapshot) => seen.push(snapshot));
    startFeed();
    for (let i = 0; i < 15; i++) await settle();
    expect(seen).toHaveLength(0);
  });
});
