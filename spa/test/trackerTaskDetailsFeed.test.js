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
const waitFor = (condition) => vi.waitFor(() => expect(condition()).toBe(true));

describe("tracker task details feed", () => {
  it("takes a current detail from the cache without a wire read", async () => {
    const detail = taskRecord(task("one", "2026-09-01T12:00:00Z"), [{ id: "tc-1" }]);
    await writeTaskRecord("dev", "project", "one", detail);
    const callRpc = vi.fn();
    const onChange = vi.fn();
    const feed = createTrackerTaskDetailsFeed({ deviceId: "dev", projectId: "project", callRpc, onChange });

    await feed.updateTasks([task("one", "2026-09-01T12:00:00Z")]);

    expect(feed.read().get("one")).toEqual(detail);
    expect(callRpc).not.toHaveBeenCalled();
    expect(onChange).toHaveBeenCalled();
    feed.dispose();
  });

  it("keeps an older cached detail visible until a refreshed record announces", async () => {
    const old = taskRecord(task("one", "2026-09-01T12:00:00Z"), [{ id: "tc-old" }]);
    await writeTaskRecord("dev", "project", "one", old);
    let answer;
    const callRpc = vi.fn(() => new Promise((resolve) => { answer = resolve; }));
    const feed = createTrackerTaskDetailsFeed({ deviceId: "dev", projectId: "project", callRpc });

    await feed.updateTasks([task("one", "2026-09-02T12:00:00Z")]);
    expect(feed.read().get("one")).toEqual(old);
    expect(callRpc).toHaveBeenCalledWith("tasks.get", { task_id: "one" });

    const fresh = taskRecord(task("one", "2026-09-02T12:00:00Z"), [{ id: "tc-new" }]);
    answer(fresh);
    await waitFor(() => feed.read().get("one")?.timeline?.[0]?.id === "tc-new");
    expect(await readTaskRecord("dev", "project", "one")).toEqual(fresh);
    feed.dispose();
  });

  it("bounds live reads and drops results after disposal", async () => {
    const answers = [];
    const callRpc = vi.fn(() => new Promise((resolve) => { answers.push(resolve); }));
    const feed = createTrackerTaskDetailsFeed({ deviceId: "dev", projectId: "project", callRpc });

    await feed.updateTasks(Array.from({ length: 7 }, (_, index) => task(`task-${index}`, "2026-09-01T12:00:00Z")));
    expect(callRpc).toHaveBeenCalledTimes(4);
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
