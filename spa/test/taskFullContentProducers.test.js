// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const owner = vi.hoisted(() => ({ session: "session-before" }));
vi.mock("../src/core/deviceReconnect.js", () => ({ deviceSession: () => owner.session }));

const deviceId = "full-content-producer", projectId = "proj-1";
const task = (id, extra = {}) => ({ id, project_id: projectId, number: 42,
  title: "Held", body: "Held full body", updated_at: "2026-10-29T00:00:00.000900Z", ...extra });
const deferred = () => {
  let resolve;
  const promise = new Promise((yes) => { resolve = yes; });
  return { promise, resolve };
};
let feed;

beforeEach(() => {
  vi.resetModules();
  owner.session = "session-before";
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});
afterEach(() => feed?.dispose());

it("refuses a full detail response from a replaced session while its listed version stays wanted", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const { createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js");
  const response = deferred();
  const callRpc = vi.fn(() => response.promise);
  feed = createTrackerTaskDetailsFeed({ deviceId, projectId, callRpc });
  await feed.updateTasks([task("one")]);
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(1));
  owner.session = "session-after";
  response.resolve({ task: task("one", { title: "Retired session", body: "Retired body" }), timeline: [] });
  // Wait for the real writer's transaction rather than inspecting a transient
  // empty cache before the deferred response has had a chance to publish.
  await new Promise((done) => setTimeout(done, 40));
  expect(await tracker.readTaskRecord(deviceId, projectId, "one")).toBeNull();
  expect(feed.read().has("one")).toBe(false);
  expect(callRpc).toHaveBeenCalledTimes(1);
});
