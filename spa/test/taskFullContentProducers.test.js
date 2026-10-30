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

it("preserves a real feed full-body dispatch observation against a native same-ms older action receipt", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const local = await import("../src/core/localCache.js");
  const { captureTaskContentRead } = await import("../src/core/taskContentAuthority.js");
  const { TASK_READ_COUNT_ADDRESS } = await import("../src/core/taskReadOrder.js");
  const { cacheTaskWaitingOnResponse } = await import("../src/core/taskWaitingOnCache.js");
  const { summaryAddress } = await import("../src/core/taskSummaryCache.js");
  const { createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js");
  const contentRead = await captureTaskContentRead({ deviceId, projectId });
  const current = task("native", { watched: false, state: "open", status: "ready" });
  let dispatched;
  const writer = vi.spyOn(tracker, "writeTaskRecord");
  const callRpc = vi.fn(async () => {
    dispatched = (await local.readCanonicalCached(TASK_READ_COUNT_ADDRESS)).value;
    return { task: current, timeline: [] };
  });
  feed = createTrackerTaskDetailsFeed({ deviceId, projectId, callRpc });
  await feed.updateTasks([current]);
  await vi.waitFor(() => expect(writer).toHaveBeenCalledTimes(1));
  await writer.mock.results[0].value;
  const before = (await tracker.readTaskRecord(deviceId, projectId, current.id)).task;
  expect(before.__task_content_cache?.body?.read_order).toBe(dispatched);
  expect(dispatched).toBeGreaterThan(contentRead.readOrder);
  await cacheTaskWaitingOnResponse(deviceId, projectId, { task: { ...current, title: "Older title", body: "Older body",
    updated_at: "2026-10-29T00:00:00.000100Z" } }, { contentRead, issuedRead: contentRead.readOrder });
  const after = (await tracker.readTaskRecord(deviceId, projectId, current.id)).task;
  expect({ title: after.title, body: after.body, updated_at: after.updated_at }).toEqual({
    title: current.title, body: current.body, updated_at: current.updated_at,
  });
  expect(after.__task_content_cache.body.read_order).toBe(dispatched);
  expect((await local.readCached(summaryAddress({ deviceId, projectId }, current.id))).value.task.title).toBe(current.title);
});

it.each(["native same-ms timestamp", "later receipt"])("admits independent feed facts across a newer held ordinary observation: %s", async (ordering) => {
  const clock = vi.spyOn(Date, "now").mockReturnValue(1800000000000);
  try {
    const tracker = await import("../src/core/trackerCache.js");
    const { captureTaskContentRead } = await import("../src/core/taskContentAuthority.js");
    const { createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js");
    const address = tracker.taskAddress(deviceId, projectId, "independent");
    const held = task("independent", { title: "Newer held title", body: "Newer held body", state: "open", status: "ready",
      watched: true, watch_started_after: "", read_through: "tc-10", unread_count: 3,
      review_summary: { task_id: "independent", version: 1, status: "open", mode: "pull_request" } });
    const seedRead = await captureTaskContentRead(address, { revalidate: "full" });
    await tracker.writeTaskRecord(deviceId, projectId, held.id, tracker.taskRecord(held, [{ id: "held-history" }]), { contentRead: seedRead });
    const response = deferred(), callRpc = vi.fn(() => response.promise);
    feed = createTrackerTaskDetailsFeed({ deviceId, projectId, callRpc });
    await feed.updateTasks([{ ...held, updated_at: "2026-10-30T00:00:00Z" }]);
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(1));
    if (ordering === "later receipt") clock.mockReturnValue(1800000001000);
    const newerRead = await captureTaskContentRead(address, { revalidate: "full" });
    await tracker.writeTaskRecord(deviceId, projectId, held.id, tracker.taskRecord(held, [{ id: "newer-history" }]), { contentRead: newerRead });
    response.resolve({ task: { ...held, title: "Older reply title", body: "Older reply body", watched: false,
      read_through: "tc-20", updated_at: ordering === "native same-ms timestamp" ? "2026-10-29T00:00:00.000100Z" : held.updated_at,
      review_summary: { task_id: held.id, version: 2, status: "closed", mode: "pull_request" } }, timeline: [{ id: "older-history" }] });
    await vi.waitFor(async () => expect((await tracker.readTaskRecord(deviceId, projectId, held.id)).task.read_through).toBe("tc-20"));
    const saved = await tracker.readTaskRecord(deviceId, projectId, held.id);
    expect(saved.task).toMatchObject({ title: held.title, body: held.body, updated_at: held.updated_at,
      watched: false, read_through: "tc-20", review_summary: { version: 2, status: "closed" } });
    expect(saved.timeline).toEqual([{ id: "newer-history" }]);
    expect(saved.task.__task_content_cache.body.read_order).toBe(newerRead.readOrder);
  } finally { clock.mockRestore(); }
});

it("adopts new proven same-timestamp headers once while suppressing repeated identical header observations", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const local = await import("../src/core/localCache.js");
  const content = await import("../src/core/taskContentAuthority.js");
  const { summaryAddress } = await import("../src/core/taskSummaryCache.js");
  const { createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js");
  const held = task("same-timestamp"), scope = { deviceId, projectId }, address = summaryAddress(scope, held.id);
  await local.writeCached(tracker.taskAddress(deviceId, projectId, held.id), tracker.taskRecord(held, []));
  const publishHeader = async (title, read_through) => {
    const contentRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    const candidate = content.observeTaskContent({ ...held, title, read_through }, { contentRead, coverage: "summary" });
    await content.mergeTaskContentRecordsTogether(scope, [address], ([prior], context) => ({
      values: [{ task: content.reconcileTaskContent(prior?.value?.task, candidate, { context }).task }], retire: false,
    }));
    return (await local.readCached(address)).value.task;
  };
  const requests = [];
  const callRpc = vi.fn(() => new Promise((_resolve, reject) => { requests.push({ reject }); }));
  feed = createTrackerTaskDetailsFeed({ deviceId, projectId, callRpc });
  await feed.updateTasks([held]);
  expect(callRpc).not.toHaveBeenCalled();
  const first = await publishHeader("First proven header", "tc-1");
  expect(first.updated_at).toBe(held.updated_at);
  await feed.updateTasks([first]);
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(1));
  requests[0].reject(new Error("First refusal"));
  await feed.updateTasks([await publishHeader("First proven header", "tc-2")]);
  expect(callRpc).toHaveBeenCalledTimes(1);
  const second = await publishHeader("Changed proven header", "tc-3");
  expect(second.updated_at).toBe(first.updated_at);
  await feed.updateTasks([second]);
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(2));
  requests[1].reject(new Error("Second refusal"));
  await feed.updateTasks([await publishHeader("Changed proven header", "tc-4")]);
  expect(callRpc).toHaveBeenCalledTimes(2);
  expect((await tracker.readTaskRecord(deviceId, projectId, held.id)).task.body).toBe(held.body);
});
