import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let paging = true;

const task = (number) => ({ id: `task-${number}`, number, updated_at: "2026-10-11T12:00:00Z" });
const gate = () => {
  let release;
  const promise = new Promise((resolve) => { release = resolve; });
  return { promise, release };
};
let phases, trackerCache;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  paging = true;
  const pages = await import("../src/core/trackerPages.js");
  vi.spyOn(pages, "pagesTasks").mockImplementation(() => paging);
  phases = await import("../src/core/syncTaskPhases.js");
  trackerCache = await import("../src/core/trackerCache.js");
});

function fixture(answers = {}) {
  const context = { deviceId: "dev-1", active: () => true };
  const ask = vi.fn(async (_context, method, params) => {
    if (answers[method]) return answers[method](params);
    if (method === "tasks.columns") return { columns: [{ id: "open" }] };
    return params.cursor ? { tasks: [task(1)] } : { tasks: [task(2)], next_cursor: "below2" };
  });
  const readReviews = vi.fn(async () => {});
  const serialize = vi.fn((_projectId, operation) => operation());
  const pass = phases.createTaskPhases(context, { ask, readReviews, serialize });
  return { context, ask, readReviews, serialize, pass };
}

describe("project tasks split around the inbox phase", () => {
  it.each([true, false])("waits for columns after a refused first list (paging=%s)", async (paged) => {
    paging = paged;
    const columns = gate();
    const { pass, ask, readReviews } = fixture({
      "tasks.columns": () => columns.promise,
      "tasks.list": () => null,
    });
    let settled = false;
    const rest = pass.rest("p1").then((answer) => { settled = true; return answer; });
    try {
      await vi.waitFor(() => expect(ask).toHaveBeenCalledTimes(2));
      expect(settled).toBe(false);
      expect(readReviews).not.toHaveBeenCalled();
    } finally {
      columns.release({ columns: [{ id: "open" }] });
      await rest;
    }
    expect(await rest).toBe(false);
    expect(await trackerCache.readTasksRecord("dev-1", "p1")).toBeNull();
  });

  it("waits for the sibling columns read before propagating a first-list failure", async () => {
    const columns = gate(), failure = new Error("cache unavailable");
    const { pass, ask } = fixture({
      "tasks.columns": () => columns.promise,
      "tasks.list": () => { throw failure; },
    });
    let settled = false;
    const rest = pass.rest("p1").catch((error) => { settled = true; return error; });
    try {
      await vi.waitFor(() => expect(ask).toHaveBeenCalledTimes(2));
      expect(settled).toBe(false);
    } finally {
      columns.release({ columns: [] });
      await rest;
    }
    expect(await rest).toBe(failure);
  });

  it("requests absent columns beside the first page and defers remaining pages and reviews", async () => {
    let release;
    const heldPage = new Promise((resolve) => { release = resolve; });
    const { pass, ask, readReviews, serialize } = fixture({
      "tasks.list": (params) => params.cursor ? { tasks: [task(1)] } : heldPage,
    });
    const first = pass.first("p1", "foreground");
    await vi.waitFor(() => expect(ask.mock.calls.map(([, method]) => method).sort()).toEqual(["tasks.columns", "tasks.list"]));
    expect(ask.mock.calls.every(([, , , priority]) => priority === "foreground")).toBe(true);
    release({ tasks: [task(2)], next_cursor: "below2" });
    await first;
    expect((await trackerCache.readTasksRecord("dev-1", "p1")).tasks.map((row) => row.number)).toEqual([2]);
    expect(readReviews).not.toHaveBeenCalled();
    await pass.rest("p1", "background");
    expect(ask.mock.calls.filter(([, method]) => method === "tasks.list").map(([, , params, priority]) => [params.cursor, priority]))
      .toEqual([[undefined, "foreground"], ["below2", "background"]]);
    expect(readReviews).toHaveBeenCalledWith(expect.any(Object), "p1", "background");
    expect(serialize).toHaveBeenCalledTimes(2);
  });

  it("keeps held columns and reads an unopened project's complete walk through rest", async () => {
    await trackerCache.writeTasksRecord("dev-1", "p1", trackerCache.tasksRecord([], [{ id: "held" }]));
    const { pass, ask } = fixture();
    await pass.rest("p1", "background");
    expect(ask.mock.calls.map(([, method]) => method)).toEqual(["tasks.list", "tasks.list"]);
    expect((await trackerCache.readTasksRecord("dev-1", "p1")).columns).toEqual([{ id: "held" }]);
  });

  it("uses a whole list once for a legacy bridge, with reviews deferred", async () => {
    paging = false;
    const { pass, ask, readReviews } = fixture();
    await pass.first("p1");
    expect(ask.mock.calls.find(([, method]) => method === "tasks.list")[2]).toEqual({ project_id: "p1" });
    expect(readReviews).not.toHaveBeenCalled();
    await pass.rest("p1", "background");
    expect(ask.mock.calls.filter(([, method]) => method === "tasks.list")).toHaveLength(1);
    expect(readReviews).toHaveBeenCalledTimes(1);
  });

  it("fences a held first page and its continuation when the pass stops", async () => {
    let release;
    const held = new Promise((resolve) => { release = resolve; });
    const { pass, context, ask, readReviews } = fixture({ "tasks.list": () => held });
    const first = pass.first("p1");
    await vi.waitFor(() => expect(ask).toHaveBeenCalledTimes(2));
    context.active = () => false;
    release({ tasks: [task(2)], next_cursor: "below2" });
    await first;
    await pass.rest("p1", "background");
    expect(await trackerCache.readTasksRecord("dev-1", "p1")).toBeNull();
    expect(ask).toHaveBeenCalledTimes(2);
    expect(readReviews).not.toHaveBeenCalled();
  });
});
