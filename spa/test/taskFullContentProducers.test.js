// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const owner = vi.hoisted(() => ({ session: "session-before", checks: 0 }));
vi.mock("../src/core/deviceReconnect.js", () => ({ deviceSession: () => { owner.checks += 1; return owner.session; } }));

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
  owner.checks = 0;
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});
afterEach(() => feed?.dispose());

it("refuses a full detail response from a replaced session while its listed version stays wanted", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const { createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js");
  const response = deferred();
  const writer = vi.spyOn(tracker, "writeTaskRecord");
  const callRpc = vi.fn(() => response.promise);
  feed = createTrackerTaskDetailsFeed({ deviceId, projectId, callRpc });
  await feed.updateTasks([task("one")]);
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(1));
  const checksBeforeReply = owner.checks;
  owner.session = "session-after";
  response.resolve({ task: task("one", { title: "Retired session", body: "Retired body" }), timeline: [] });
  // Observe the retired owner check or await the actual writer; a temporary
  // empty cache while the producer still awaits work is not evidence.
  await vi.waitFor(() => expect(writer.mock.calls.length > 0 || owner.checks > checksBeforeReply).toBe(true));
  if (writer.mock.results[0]) await writer.mock.results[0].value;
  expect(await tracker.readTaskRecord(deviceId, projectId, "one")).toBeNull();
  expect(feed.read().has("one")).toBe(false);
  expect(callRpc).toHaveBeenCalledTimes(1);
});

it("revalidates explicit body uncertainty once without sweeping unchanged legacy details or exceeding four reads", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const local = await import("../src/core/localCache.js");
  const { mergeTaskContentRecordsTogether, retireTaskContent } = await import("../src/core/taskContentAuthority.js");
  const { createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js");
  const tasks = Array.from({ length: 5 }, (_, i) => task(`task-${i}`, { number: i + 1 }));
  const address = (id) => tracker.taskAddress(deviceId, projectId, id);
  for (const row of tasks) await local.writeCached(address(row.id), tracker.taskRecord(row, []));
  const requests = [];
  const callRpc = vi.fn(() => new Promise((_resolve, reject) => { requests.push({ reject }); }));
  const onChange = vi.fn();
  feed = createTrackerTaskDetailsFeed({ deviceId, projectId, callRpc, onChange });
  await feed.updateTasks(tasks);
  expect(callRpc).not.toHaveBeenCalled();
  for (const row of tasks) await mergeTaskContentRecordsTogether({ deviceId, projectId }, [address(row.id)], ([held]) => ({
    values: [{ ...held.value, task: retireTaskContent(held.value.task, "body") }], retire: true,
  }));
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(4));
  requests[0].reject(new Error("Offline"));
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(5));
  for (let i = 0; i < 3; i += 1) {
    const seen = onChange.mock.calls.length;
    await local.mergeCachedAtomically(address(tasks[0].id), (held) => ({ ...held,
      task: { ...held.task, read_through: `tc-${i}` },
    }));
    await vi.waitFor(() => expect(onChange.mock.calls.length).toBeGreaterThan(seen));
    expect(callRpc).toHaveBeenCalledTimes(5);
  }
  feed.dispose();
  for (const request of requests.slice(1)) request.reject(new Error("Disposed"));
});
