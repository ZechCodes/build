// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let createTrackerTaskDetailsFeed, taskRecord, readTaskRecord, writeTaskRecord;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js"));
  ({ taskRecord, readTaskRecord, writeTaskRecord } = await import("../src/core/trackerCache.js"));
});

const task = (id, updated_at) => ({ id, updated_at });
const fullTask = (id, updated_at) => ({ ...task(id, updated_at), project_id: "project", number: 1,
  title: "Saved task", state: "open", status: "ready", body: "Same body", attachments: [],
  watched: false, read_through: null, dismissed_through: null });
const publicTask = ({ __task_content_cache, ...value }) => value;
const waitFor = (condition) => vi.waitFor(() => expect(condition()).toBe(true));

describe("tracker task details feed", () => {
  it("takes a current detail from the cache without a wire read", async () => {
    const detail = taskRecord(fullTask("one", "2026-09-01T12:00:00Z"), [{ id: "tc-1" }]);
    const { writeCached } = await import("../src/core/localCache.js");
    const { taskAddress } = await import("../src/core/trackerCache.js");
    // This is a migrated, unmarked legacy record, not a new blind mutation.
    await writeCached(taskAddress("dev", "project", "one"), detail);
    const callRpc = vi.fn();
    const onChange = vi.fn();
    const feed = createTrackerTaskDetailsFeed({ deviceId: "dev", projectId: "project", callRpc, onChange });

    await feed.updateTasks([task("one", "2026-09-01T12:00:00Z")]);

    expect(feed.read().get("one")).toEqual(detail);
    expect(feed.read().get("one").task).not.toHaveProperty("__task_content_cache");
    expect(callRpc).not.toHaveBeenCalled();
    expect(onChange).toHaveBeenCalled();
    feed.dispose();
  });

  it("keeps an older cached detail visible until a refreshed record announces", async () => {
    const old = taskRecord(fullTask("one", "2026-09-01T12:00:00Z"), [{ id: "tc-old" }]);
    const { writeCached } = await import("../src/core/localCache.js");
    const { taskAddress } = await import("../src/core/trackerCache.js");
    await writeCached(taskAddress("dev", "project", "one"), old);
    let answer;
    const callRpc = vi.fn(() => new Promise((resolve) => { answer = resolve; }));
    const feed = createTrackerTaskDetailsFeed({ deviceId: "dev", projectId: "project", callRpc });

    await feed.updateTasks([task("one", "2026-09-02T12:00:00Z")]);
    expect(feed.read().get("one")).toEqual(old);
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("tasks.get", { task_id: "one" }));

    const fresh = taskRecord(fullTask("one", "2026-09-02T12:00:00Z"), [{ id: "tc-new" }]);
    answer(fresh);
    await waitFor(() => feed.read().get("one")?.timeline?.[0]?.id === "tc-new");
    const saved = await readTaskRecord("dev", "project", "one");
    expect({ ...saved, task: publicTask(saved.task) }).toEqual(fresh);
    expect(feed.read().get("one")).toEqual(saved);
    expect(saved.task.__task_content_cache.body.read_order).toBeGreaterThan(0);
    feed.dispose();
  });

  it("bounds live reads and drops results after disposal", async () => {
    const answers = [];
    const callRpc = vi.fn(() => new Promise((resolve) => { answers.push(resolve); }));
    const feed = createTrackerTaskDetailsFeed({ deviceId: "dev", projectId: "project", callRpc });

    await feed.updateTasks(Array.from({ length: 7 }, (_, index) => task(`task-${index}`, "2026-09-01T12:00:00Z")));
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(4));
    answers[0]({ task: task("task-0", "2026-09-01T12:00:00Z"), timeline: [] });
    await waitFor(() => callRpc.mock.calls.length === 5);

    feed.dispose();
    answers[1]({ task: task("task-1", "2026-09-01T12:00:00Z"), timeline: [] });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(await readTaskRecord("dev", "project", "task-1")).toBeNull();
    expect(callRpc).toHaveBeenCalledTimes(5);
  });

  it("ignores a malformed answer and does not repeat a failed version", async () => {
    const callRpc = vi.fn(async () => ({}));
    const feed = createTrackerTaskDetailsFeed({ deviceId: "dev", projectId: "project", callRpc });
    const listed = [task("one", "2026-09-01T12:00:00Z")];

    await feed.updateTasks(listed);
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(1));
    await feed.updateTasks(listed);

    expect(await readTaskRecord("dev", "project", "one")).toBeNull();
    expect(callRpc).toHaveBeenCalledTimes(1);
    feed.dispose();
  });
});

it("fences a pending detail feed read against a same-stamp saved assignment mutation", async () => {
  const clock = vi.spyOn(Date, "now").mockReturnValue(123456);
  const tracker = await import("../src/core/trackerCache.js");
  const waiting = await import("../src/core/taskWaitingOnCache.js");
  const { captureTaskContentRead } = await import("../src/core/taskContentAuthority.js");
  const old = taskRecord(fullTask("one", "2026-09-01T12:00:00Z"), []);
  const contentRead = await captureTaskContentRead(tracker.taskAddress("dev", "project", "one"), { revalidate: "full" });
  await writeTaskRecord("dev", "project", "one", old, { contentRead });
  let resolve;
  const callRpc = vi.fn(() => new Promise((answer) => { resolve = answer; }));
  const feed = createTrackerTaskDetailsFeed({ deviceId: "dev", projectId: "project", callRpc });
  try {
    await feed.updateTasks([task("one", "2026-09-02T12:00:00Z")]);
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(1));
    const next = { ...old.task, dependencies: { blocked: true, waiting_on: [{ task_id: "prerequisite" }] }, deferred_assignment: { id: "saved", state: "waiting" } };
    const actionRead = await captureTaskContentRead({ deviceId: "dev", projectId: "project" });
    await waiting.cacheTaskWaitingOnResponse("dev", "project", { task: next }, {
      contentRead: actionRead, issuedRead: actionRead.readOrder,
    });
    expect(publicTask((await readTaskRecord("dev", "project", "one")).task)).toEqual(next);
    const admission = vi.spyOn(tracker, "writeTaskRecord");
    resolve(old);
    await vi.waitFor(() => expect(admission).toHaveBeenCalled());
    expect(await admission.mock.results[0].value).toBe(false);
    expect(publicTask((await readTaskRecord("dev", "project", "one")).task)).toEqual(next);
  } finally {
    feed.dispose();
    clock.mockRestore();
  }
});
