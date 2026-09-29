// #229: what fills the reference index — the feed as it is delivered, and each
// project's task list as the tracker writes it to the cache. Nothing is read
// off the wire; a list written after the feed lands still reaches the index.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache;
let tracker;
let index;
let feedIndex;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  index = await import("../src/core/referenceIndex.js");
  ({ feedReferenceIndex: feedIndex } = await import("../src/core/referenceIndexFeed.js"));
});

let stop = () => {};
afterEach(() => stop());

/** A feed that delivers when told, the way core/taskFeed.js does. */
function feedSource() {
  const listeners = new Set();
  return {
    subscribeFeed(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    deliver: (feed) => listeners.forEach((listener) => listener(feed)),
  };
}

const feed = {
  projects: [{ id: "proj-1", deviceId: "dev-1", projectKey: "dev-1/proj-1", name: "Build" }],
  workspaces: [{ id: "ws-1", workspace_id: "ws-1", name: "tasks-spa", projectKey: "dev-1/proj-1" }],
  items: [],
};
const place = { deviceId: "dev-1", projectId: "proj-1" };
const settled = () => vi.waitFor(() => new Promise((resolve) => setTimeout(resolve, 0)));

describe("the reference index's feed", () => {
  it("answers workspaces as soon as the feed is delivered", () => {
    const source = feedSource();
    stop = feedIndex({ subscribeFeed: source.subscribeFeed });
    source.deliver(feed);
    expect(index.referenceResolver({ place }).workspace("tasks-spa")?.workspaceId).toBe("ws-1");
  });

  it("reads each project's cached task list, and hears a later write", async () => {
    await cache.writeCached(tracker.tasksAddress("dev-1", "proj-1"), { tasks: [{ id: "task-1", number: 1, title: "One" }] });
    const source = feedSource();
    stop = feedIndex({ subscribeFeed: source.subscribeFeed });
    source.deliver(feed);
    await vi.waitFor(() => expect(index.referenceResolver({ place }).task(1)?.taskId).toBe("task-1"));

    await cache.writeCached(tracker.tasksAddress("dev-1", "proj-1"), { tasks: [{ id: "task-2", number: 2, title: "Two" }] });
    await vi.waitFor(() => expect(index.referenceResolver({ place }).task(2)?.taskId).toBe("task-2"));
    await settled();
  });

  it("stops hearing anything once stopped", async () => {
    const source = feedSource();
    stop = feedIndex({ subscribeFeed: source.subscribeFeed });
    stop();
    source.deliver(feed);
    expect(index.referenceResolver({ place }).workspace("tasks-spa")).toBeUndefined();
  });
});
