import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let summaries;
const scope = { deviceId: "markers-device", projectId: "proj-1" };
const fields = ["read_through", "watch_started_after", "dismissed_through"];
const row = (mark, count) => ({ id: "task-1", title: "One", unread_count: count,
  ...Object.fromEntries(fields.map((field) => [field, mark])) });
async function land(generation, task) {
  const ticket = await summaries.beginSummaryQuery(scope, {}, generation);
  return summaries.landSummaryPage(ticket, { summary: true, basis: `B${generation}`, tasks: [task] }, { read: generation });
}

beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  summaries = await import("../src/core/taskSummaryCache.js");
});

it.each(["", " ", "\u0000", "withoutprefix", "other-a-b", "tc-\u{10000}"])(
  "retains exact present opaque floor %j across a later absent response", async (mark) => {
    await land(1, row(mark, 0)); await land(2, row(null, 9));
    const task = (await summaries.readSummaryQuery(scope, {})).tasks[0];
    for (const field of fields) expect(task[field]).toBe(mark);
    expect(task.unread_count).toBe(0);
  },
);

it.each([["tc-a-b", "te-a-c"], ["tc-\ue000", "te-\u{10000}"], [null, ""]])(
  "advances canonical suffix floor %j to %j independently of dispatch order", async (before, after) => {
    await land(3, row(before, 7));
    const ticket = await summaries.beginSummaryQuery(scope, { states: ["open"] }, 1);
    await summaries.landSummaryPage(ticket, { summary: true, basis: "B3", tasks: [row(after, 0)] }, { read: 1 });
    const task = (await summaries.readSummaryQuery(scope, {})).tasks[0];
    for (const field of fields) expect(task[field]).toBe(after);
    expect(task.unread_count).toBe(0);
  },
);

it("uses fresh counters for equal-suffix markers with different prefixes", async () => {
  await land(1, { ...row("tc-A", 1), updated_at: "2026-10-29T00:00:01Z" });
  await land(2, { ...row("te-A", 2), updated_at: "2026-10-29T00:00:02Z" });
  const held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0]).toMatchObject({ read_through: "tc-A", unread_count: 2 });
  expect(held.stale).toBe(false);
});

it("selects the counters whose combined read/watch floor covers crossed fields", async () => {
  await land(1, { ...row(null, 5), read_through: "tc-10", watch_started_after: "te-15" });
  await land(2, { ...row(null, 4), read_through: "tc-20", watch_started_after: "te-05" });
  const held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0]).toMatchObject({ read_through: "tc-20", watch_started_after: "te-15", unread_count: 4 });
  expect(held.stale).toBe(false);
});

it("requires refresh when retaining older counters against a newer content snapshot", async () => {
  await land(1, { ...row("tc-20", 1), updated_at: "2026-10-29T00:00:01Z" });
  await land(2, { ...row("tc-10", 4), updated_at: "2026-10-29T00:00:02Z" });
  const held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0].unread_count).toBe(1);
  expect(held.stale).toBe(true);
});

it("keeps attention counter authority separate from the newer watch floor", async () => {
  await land(1, { ...row(null, 2), read_through: "tc-20", watch_started_after: "te-05",
    unread_mentions: 0, unread_agent_comments: 0, updated_at: "2026-10-29T00:00:01Z" });
  await land(2, { ...row(null, 0), read_through: "tc-10", watch_started_after: "te-30",
    unread_mentions: 3, unread_agent_comments: 3, updated_at: "2026-10-29T00:00:02Z" });
  const held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0]).toMatchObject({ read_through: "tc-20", watch_started_after: "te-30",
    unread_count: 0, unread_mentions: 0, unread_agent_comments: 0 });
  expect(held.stale).toBe(true);
});

it("ends the watch lineage on authoritative unwatch and rejects its later resurrection", async () => {
  await land(1, { ...row("tc-10", 1), watched: true, watch_started_after: "te-30" });
  const unwatched = { id: "task-1", title: "One", watched: false, read_through: "tc-10" };
  await land(2, unwatched); await land(3, unwatched);
  let held = await summaries.readSummaryQuery(scope, {});
  expect(held.stale).toBe(false);
  expect(held.tasks[0]).not.toHaveProperty("watch_started_after");
  expect(held.tasks[0]).not.toHaveProperty("unread_count");
  const ticket = await summaries.beginSummaryQuery(scope, { states: ["open"] }, 1);
  await summaries.landSummaryPage(ticket, { summary: true, basis: "B1", tasks: [
    { ...row("tc-10", 1), watched: true, watch_started_after: "te-30" },
  ] }, { read: 1 });
  held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0].watched).toBe(false); expect(held.stale).toBe(false);
  await land(4, { ...row("tc-10", 2), watched: true, watch_started_after: "" });
  held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0]).toMatchObject({ watched: true, watch_started_after: "", unread_count: 2 });
  expect(held.stale).toBe(false);
});
