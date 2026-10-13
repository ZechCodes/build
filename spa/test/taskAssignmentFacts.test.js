import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { taskAddress, tasksAddress, tasksQueryAddress, tasksPageAddress } from "../src/core/trackerCache.js";

let readTaskAssignmentFacts, writeCached;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ writeCached } = await import("../src/core/localCache.js"));
  ({ readTaskAssignmentFacts } = await import("../src/core/taskAssignmentFacts.js"));
});
afterEach(() => vi.restoreAllMocks());
const row = (blocked, updated_at = "2026-10-13T01:00:00Z") => ({ id: "task", updated_at, dependencies: { blocked } });
const read = () => readTaskAssignmentFacts("device", "project", "task");
const lists = [
  ["catalogue", tasksAddress("device", "project")],
  ["query", tasksQueryAddress("device", "project", { state: "open" })],
  ["page", tasksPageAddress("device", "project", { cursor: "next" })],
];

describe("assignment facts across cached task replicas", () => {
  it.each(lists)("uses a newer %s reblock while detail is still ready", async (_kind, address) => {
    await writeCached(taskAddress("device", "project", "task"), { task: row(false) });
    await writeCached(address, { tasks: [row(true, "2026-10-13T01:01:00Z")] });
    expect((await read()).dependencies.blocked).toBe(true);
  });

  it.each(lists)("ignores an older %s blocker even when it arrived after ready detail", async (_kind, address) => {
    await writeCached(taskAddress("device", "project", "task"), { task: row(false, "2026-10-13T01:01:00Z") });
    await writeCached(address, { tasks: [row(true)] });
    expect((await read()).dependencies.blocked).toBe(false);
  });

  it("keeps waiting when timestamps tie, regardless of cache write order", async () => {
    vi.spyOn(Date, "now").mockReturnValue(1_800_000_000_000);
    await writeCached(taskAddress("device", "project", "task"), { task: row(false) });
    await writeCached(lists[1][1], { tasks: [row(true)] });
    expect((await read()).dependencies.blocked).toBe(true);
    await writeCached(taskAddress("device", "project", "task"), { task: row(false) });
    expect((await read()).dependencies.blocked).toBe(true);
  });

  it.each(lists.slice(0, 2))("keeps newer %s facts when a same-stamp older page lands late", async (_kind, address) => {
    const now = vi.spyOn(Date, "now").mockReturnValue(1_800_000_000_000);
    await writeCached(address, { tasks: [row(true)], read_order: 1_800_000_000_002 });
    now.mockReturnValue(1_800_000_000_010);
    await writeCached(lists[2][1], { tasks: [row(false)], read_order: 1_800_000_000_001 });
    expect((await read()).dependencies.blocked).toBe(true);
  });

  it("keeps a delayed same-stamp blocker despite a later ready query read", async () => {
    await writeCached(lists[1][1], { tasks: [row(false)], read_order: 1_800_000_000_002 });
    await writeCached(lists[2][1], { tasks: [row(true)], read_order: 1_800_000_000_001 });
    expect((await read()).dependencies.blocked).toBe(true);
  });

  it.each([undefined, "invalid"])("keeps waiting when either source has timestamp %s", async (stamp) => {
    for (const blocked of [true, false]) {
      await writeCached(taskAddress("device", "project", "task"), { task: { ...row(blocked), updated_at: stamp } });
      await writeCached(lists[1][1], { tasks: [row(!blocked, "2026-10-13T01:01:00Z")] });
      expect((await read()).dependencies.blocked).toBe(true);
    }
  });

  it("discards older stamped blockers before resolving unknown timestamps", async () => {
    await writeCached(lists[0][1], { tasks: [row(true)] });
    await writeCached(lists[1][1], { tasks: [row(false, "2026-10-13T01:01:00Z")] });
    await writeCached(taskAddress("device", "project", "task"), { task: { ...row(false), updated_at: undefined } });
    expect((await read()).dependencies.blocked).toBe(false);
  });

  it("uses the task timestamp before list read order", async () => {
    await writeCached(lists[0][1], { tasks: [row(true)], read_order: 1_800_000_000_002 });
    await writeCached(lists[2][1], {
      tasks: [row(false, "2026-10-13T01:01:00Z")], read_order: 1_800_000_000_001,
    });
    expect((await read()).dependencies.blocked).toBe(false);
  });

  it("does not read another device, project or task", async () => {
    await writeCached(tasksAddress("other-device", "project"), { tasks: [row(true)] });
    await writeCached(tasksAddress("device", "other-project"), { tasks: [row(true)] });
    await writeCached(tasksAddress("device", "project"), { tasks: [{ ...row(true), id: "other-task" }] });
    expect(await read()).toBeNull();
  });
});
