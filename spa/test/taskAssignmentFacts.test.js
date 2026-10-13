import { beforeEach, describe, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readTaskAssignmentFacts } from "../src/core/taskAssignmentFacts.js";
import { writeCached } from "../src/core/localCache.js";
import { taskAddress, tasksAddress, tasksQueryAddress, tasksPageAddress } from "../src/core/trackerCache.js";

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});
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

  it("uses cache write order when task timestamps tie", async () => {
    await writeCached(taskAddress("device", "project", "task"), { task: row(false) });
    await writeCached(lists[1][1], { tasks: [row(true)] });
    expect((await read()).dependencies.blocked).toBe(true);
    await writeCached(taskAddress("device", "project", "task"), { task: row(false) });
    expect((await read()).dependencies.blocked).toBe(false);
  });

  it("does not read another device, project or task", async () => {
    await writeCached(tasksAddress("other-device", "project"), { tasks: [row(true)] });
    await writeCached(tasksAddress("device", "other-project"), { tasks: [row(true)] });
    await writeCached(tasksAddress("device", "project"), { tasks: [{ ...row(true), id: "other-task" }] });
    expect(await read()).toBeNull();
  });
});
