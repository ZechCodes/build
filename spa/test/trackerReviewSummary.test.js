import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readFileSync } from "node:fs";

const fixture = (name) => JSON.parse(readFileSync(new URL(`../../fixtures/api/v1/${name}.json`, import.meta.url), "utf8"));
const task = fixture("tasks.get").examples[0].result.task;
const workspace = fixture("workspace.list").examples.find(({ result }) => result.workspaces[0]?.active_review).result.workspaces[0];
const DEVICE = "review-device";
const PROJECT = task.project_id;
const PARAMS = { project_id: PROJECT, state: "open" };
const summary = (version, over = {}) => ({ ...task.review_summary, version, ...over });
const row = (version, over = {}) => ({ ...task, review_summary: summary(version), ...over });
const withoutSummary = (over = {}) => {
  const next = { ...task, ...over };
  delete next.review_summary;
  return next;
};

let cache, tracker, pages, sessions;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  pages = await import("../src/core/trackerPages.js");
  sessions = await import("../src/core/sessionListCache.js");
});

describe("PR summary version floors", () => {
  it("takes other task fields from the reply while preserving a newer review summary", () => {
    const held = row(7, { title: "held" });
    const incoming = row(3, { title: "answered", updated_at: "2026-10-08T20:00:00Z" });
    expect(tracker.preserveReviewSummary(held, incoming)).toEqual({ ...incoming, review_summary: held.review_summary });
    expect(incoming.review_summary.version).toBe(3);
  });

  it("takes equal and newer versions, keeping legacy task rows unchanged", () => {
    expect(tracker.preserveReviewSummary(row(3), row(3, { review_summary: summary(3, { status: "closed" }) })).review_summary.status).toBe("closed");
    expect(tracker.preserveReviewSummary(row(3), row(4)).review_summary.version).toBe(4);
    const legacy = withoutSummary();
    expect(tracker.preserveReviewSummary(legacy, legacy)).toBe(legacy);
  });

  it("retains the known summary over an older absent reply and accepts a demonstrably newer absence", () => {
    const held = row(7);
    expect(tracker.preserveReviewSummary(held, withoutSummary()).review_summary).toEqual(held.review_summary);
    expect(tracker.preserveReviewSummary(held, withoutSummary({ updated_at: "2026-10-08T20:00:00Z" }))).not.toHaveProperty("review_summary");
  });

  it("does not carry a floor between different task identities or nonnumeric versions", () => {
    const differentTask = row(1, { id: "task-other", review_summary: summary(1, { task_id: "task-other" }) });
    expect(tracker.preserveReviewSummary(row(7), differentTask)).toBe(differentTask);
    const unknownVersion = row("2");
    expect(tracker.preserveReviewSummary(row(7), unknownVersion)).toBe(unknownVersion);
  });

  it("preserves workspace summaries only for the same PR task", () => {
    const held = { ...workspace, active_review: { ...workspace.active_review, version: 7 } };
    const older = { ...workspace, active_review: { ...workspace.active_review, version: 2 } };
    expect(tracker.preserveReviewSummary(held, older, "active_review").active_review.version).toBe(7);
    const nextReview = { ...older, active_review: { ...older.active_review, task_id: "new-pr-task", version: 1 } };
    expect(tracker.preserveReviewSummary(held, nextReview, "active_review")).toBe(nextReview);
  });

  it("maps only incoming list membership and matches held task rows by identity", () => {
    const incoming = [row(3), row(1, { id: "task-new", review_summary: summary(1, { task_id: "task-new" }) })];
    const held = [row(7), row(8, { id: "task-removed" })];
    const merged = tracker.preserveReviewSummaries(held, incoming);
    expect(merged.map(({ id }) => id)).toEqual(incoming.map(({ id }) => id));
    expect(merged.map(({ review_summary }) => review_summary.version)).toEqual([7, 1]);
  });

  it("keeps clear authority when cached task copies are merged in either order", () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const visible = tracker.preserveTaskReviewSummary(null, row(7, { updated_at: at(20) }));
    const cleared = tracker.preserveTaskReviewSummary(visible, withoutSummary({ updated_at: at(30) }));
    const stale = tracker.preserveTaskReviewSummary(cleared, row(3, { title: "ordinary stale copy", updated_at: at(10) }));
    for (const [held, incoming] of [[visible, stale], [stale, visible]]) {
      const merged = tracker.preserveTaskReviewSummary(held, incoming);
      expect(merged).not.toHaveProperty("review_summary");
      expect(tracker.preserveTaskReviewSummary(merged, row(7, { updated_at: at(25) }))).not.toHaveProperty("review_summary");
    }
    expect(stale.title).toBe("ordinary stale copy");
  });

  it("fences a higher cached PR version observed before a clear, then admits a fresh raw advance", () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const seed = tracker.preserveTaskReviewSummary(null, row(7, { updated_at: at(20) }), { readOrder: 20 });
    const clear = tracker.preserveTaskReviewSummary(seed, withoutSummary({ updated_at: at(30) }), { readOrder: 30 });
    const visible = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(10) }), { readOrder: 10 });
    for (const [held, incoming] of [[clear, visible], [visible, clear]]) {
      const merged = tracker.preserveTaskReviewSummary(held, incoming);
      expect(merged).not.toHaveProperty("review_summary");
      expect(tracker.preserveTaskReviewSummary(merged, row(8, { updated_at: at(10) }), { readOrder: 10 })).not.toHaveProperty("review_summary");
      expect(tracker.preserveTaskReviewSummary(merged, row(8, { updated_at: at(10) }), { readOrder: 40 }).review_summary.version).toBe(8);
    }
  });

  it.each([
    { name: "known read orders", visibleRead: 10, clearRead: 30, staleRead: 40, staleMinute: 5, refreshRead: 50 },
    { name: "unknown read orders and a newer ordinary timestamp", visibleRead: 0, clearRead: 0, staleRead: 0, staleMinute: 40, refreshRead: 0 },
  ])("does not refresh accepted PR authority after a rejected summary with $name", ({ visibleRead, clearRead, staleRead, staleMinute, refreshRead }) => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const seed = tracker.preserveTaskReviewSummary(null, row(7, { updated_at: at(20) }));
    const clear = tracker.preserveTaskReviewSummary(seed, withoutSummary({ updated_at: at(30) }), { readOrder: clearRead });
    const visible = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(10) }), { readOrder: visibleRead });
    const rejected = tracker.preserveTaskReviewSummary(visible, row(3, { title: "ordinary rejected reply", updated_at: at(staleMinute) }), { readOrder: staleRead });
    expect(rejected).toMatchObject({ title: "ordinary rejected reply", review_summary: summary(8) });
    expect(tracker.preserveTaskReviewSummary(clear, rejected)).not.toHaveProperty("review_summary");
    expect(tracker.preserveTaskReviewSummary(rejected, clear)).not.toHaveProperty("review_summary");
    for (const version of [8, 9]) {
      const refreshed = tracker.preserveTaskReviewSummary(rejected, row(version, { updated_at: at(35) }), { readOrder: refreshRead });
      expect(tracker.preserveTaskReviewSummary(clear, refreshed).review_summary.version).toBe(version);
      expect(tracker.preserveTaskReviewSummary(refreshed, clear).review_summary.version).toBe(version);
    }
  });

  it("does not treat legacy protective PR clocks as a newer accepted visible observation", () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const seed = tracker.preserveTaskReviewSummary(null, row(7, { updated_at: at(20) }));
    const clear = tracker.preserveTaskReviewSummary(seed, withoutSummary({ updated_at: at(30) }), { readOrder: 30 });
    const visible = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(10) }), { readOrder: 10 });
    const inflated = tracker.preserveTaskReviewSummary(visible, row(3, { updated_at: at(40) }), { readOrder: 40 });
    const legacy = { ...inflated, __review_summary_cache: { ...inflated.__review_summary_cache } };
    delete legacy.__review_summary_cache.visible_observation;
    expect(tracker.preserveTaskReviewSummary(clear, legacy)).not.toHaveProperty("review_summary");
    expect(tracker.preserveTaskReviewSummary(legacy, clear)).not.toHaveProperty("review_summary");
    const refreshed = tracker.preserveTaskReviewSummary(legacy, row(8, { updated_at: at(35) }), { readOrder: 50 });
    expect(tracker.preserveTaskReviewSummary(clear, refreshed).review_summary.version).toBe(8);
    expect(tracker.preserveTaskReviewSummary(refreshed, clear).review_summary.version).toBe(8);
  });

  it("selects a genuinely refreshed cached equal version over rejected protective clocks", () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const seed = tracker.preserveTaskReviewSummary(null, row(7, { updated_at: at(20) }));
    const clear = tracker.preserveTaskReviewSummary(seed, withoutSummary({ updated_at: at(30) }), { readOrder: 30 });
    const old = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(10) }), { readOrder: 10 });
    const rejected = tracker.preserveTaskReviewSummary(old, row(3, { updated_at: at(40) }), { readOrder: 40 });
    const refreshed = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(5) }), { readOrder: 35 });
    for (const [held, incoming] of [[rejected, refreshed], [refreshed, rejected]]) {
      const merged = tracker.preserveTaskReviewSummary(held, incoming);
      expect(tracker.preserveTaskReviewSummary(clear, merged).review_summary.version).toBe(8);
      expect(tracker.preserveTaskReviewSummary(merged, clear).review_summary.version).toBe(8);
    }
  });

  it("compares accepted clear authority before later rejected protective clocks", () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const seed = tracker.preserveTaskReviewSummary(null, row(7, { updated_at: at(20) }), { readOrder: 20 });
    const clear = tracker.preserveTaskReviewSummary(seed, withoutSummary({ updated_at: at(30) }), { readOrder: 30 });
    const rejected = tracker.preserveTaskReviewSummary(clear, row(3, { updated_at: at(40) }), { readOrder: 40 });
    for (const version of [7, 8]) {
      const visible = tracker.preserveTaskReviewSummary(null, row(version, { updated_at: at(35) }), { readOrder: 35 });
      expect(tracker.preserveTaskReviewSummary(rejected, visible).review_summary.version).toBe(version);
      expect(tracker.preserveTaskReviewSummary(visible, rejected).review_summary.version).toBe(version);
    }
  });

  it("records the incoming accepted absence after a rejected newer task timestamp", () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const visible = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(10) }), { readOrder: 10 });
    const rejected = tracker.preserveTaskReviewSummary(visible, row(3, { updated_at: at(40) }), { readOrder: 40 });
    const clear = tracker.preserveTaskReviewSummary(rejected, withoutSummary({ updated_at: at(35) }), { readOrder: 50, allowMissing: true });
    const laterVisible = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(38) }));
    expect(tracker.preserveTaskReviewSummary(clear, laterVisible).review_summary.version).toBe(8);
    expect(tracker.preserveTaskReviewSummary(laterVisible, clear).review_summary.version).toBe(8);
  });

  it.each([[0, 0], [0, 30], [10, 0]])("uses clear timestamps when cached read orders %s and %s are incomparable", (visibleRead, clearRead) => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const seed = tracker.preserveTaskReviewSummary(null, row(7, { updated_at: at(20) }));
    const clear = tracker.preserveTaskReviewSummary(seed, withoutSummary({ updated_at: at(30) }), { readOrder: clearRead });
    const visible = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(30) }), { readOrder: visibleRead });
    const laterVisible = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(40) }), { readOrder: visibleRead });
    for (const [held, incoming] of [[clear, visible], [visible, clear]]) {
      const merged = tracker.preserveTaskReviewSummary(held, incoming);
      expect(merged).not.toHaveProperty("review_summary");
      expect(tracker.preserveTaskReviewSummary(merged, row(8, { updated_at: at(10) })).review_summary.version).toBe(8);
    }
    expect(tracker.preserveTaskReviewSummary(clear, laterVisible).review_summary.version).toBe(8);
    expect(tracker.preserveTaskReviewSummary(laterVisible, clear).review_summary.version).toBe(8);
  });

  it("keeps numeric version authority for visible cached PRs and later observed advances", () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const visibleSeven = tracker.preserveTaskReviewSummary(null, row(7, { updated_at: at(30) }), { readOrder: 30 });
    const visibleEight = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(10) }), { readOrder: 10 });
    expect(tracker.preserveTaskReviewSummary(visibleSeven, visibleEight).review_summary.version).toBe(8);
    expect(tracker.preserveTaskReviewSummary(visibleEight, visibleSeven).review_summary.version).toBe(8);
    const clear = tracker.preserveTaskReviewSummary(visibleSeven, withoutSummary({ updated_at: at(35) }), { readOrder: 35 });
    const laterEight = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(10) }), { readOrder: 40 });
    expect(tracker.preserveTaskReviewSummary(clear, laterEight).review_summary.version).toBe(8);
    expect(tracker.preserveTaskReviewSummary(laterEight, clear).review_summary.version).toBe(8);
  });

  it("combines two cached clears without lowering their numeric, timestamp or read floors", () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const seven = tracker.preserveTaskReviewSummary(null, row(7, { updated_at: at(20) }), { readOrder: 20 });
    const eight = tracker.preserveTaskReviewSummary(null, row(8, { updated_at: at(5) }), { readOrder: 5 });
    const clearSeven = tracker.preserveTaskReviewSummary(seven, withoutSummary({ updated_at: at(30) }), { readOrder: 30 });
    const clearEight = tracker.preserveTaskReviewSummary(eight, withoutSummary({ updated_at: at(10) }), { readOrder: 10 });
    for (const [held, incoming] of [[clearSeven, clearEight], [clearEight, clearSeven]]) {
      const merged = tracker.preserveTaskReviewSummary(held, incoming);
      expect(merged).not.toHaveProperty("review_summary");
      expect(tracker.preserveTaskReviewSummary(merged, row(7, { updated_at: at(40) }), { readOrder: 40 })).not.toHaveProperty("review_summary");
      expect(tracker.preserveTaskReviewSummary(merged, row(8, { updated_at: at(20) }), { readOrder: 40 })).not.toHaveProperty("review_summary");
      expect(tracker.preserveTaskReviewSummary(merged, row(9, { updated_at: at(5) }), { readOrder: 20 })).not.toHaveProperty("review_summary");
      expect(tracker.preserveTaskReviewSummary(merged, row(9, { updated_at: at(5) }), { readOrder: 40 }).review_summary.version).toBe(9);
    }
  });

  it("retains a raw PR advance over the exact clear it already observed", () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const seed = tracker.preserveTaskReviewSummary(null, row(7, { updated_at: at(20) }));
    const clear = tracker.preserveTaskReviewSummary(seed, withoutSummary({ updated_at: at(30) }));
    const accepted = tracker.preserveTaskReviewSummary(clear, row(8, { updated_at: at(10) }));
    expect(accepted.review_summary.version).toBe(8);
    const staleWrite = tracker.preserveTaskReviewSummary(accepted, row(3, { updated_at: at(5) }));
    const laterAdvance = tracker.preserveTaskReviewSummary(staleWrite, row(9, { updated_at: at(15) }));
    const staleClearWrite = tracker.preserveTaskReviewSummary(clear, row(3, { updated_at: at(50) }), { readOrder: 40 });
    for (const visible of [accepted, staleWrite, laterAdvance]) {
      expect(tracker.preserveTaskReviewSummary(clear, visible).review_summary.version).toBe(visible.review_summary.version);
      expect(tracker.preserveTaskReviewSummary(visible, clear).review_summary.version).toBe(visible.review_summary.version);
      expect(tracker.preserveTaskReviewSummary(staleClearWrite, visible).review_summary.version).toBe(visible.review_summary.version);
      expect(tracker.preserveTaskReviewSummary(visible, staleClearWrite).review_summary.version).toBe(visible.review_summary.version);
    }
  });

  it("limits an accepted PR's clear observation to that exact clear", () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const seed = tracker.preserveTaskReviewSummary(null, row(7, { updated_at: at(20) }));
    const clear = tracker.preserveTaskReviewSummary(seed, withoutSummary({ updated_at: at(30) }));
    const accepted = tracker.preserveTaskReviewSummary(clear, row(8, { updated_at: at(10) }));
    const newerSameFloorClear = tracker.preserveTaskReviewSummary(clear, withoutSummary({ updated_at: at(40) }));
    const differentlyObservedClear = tracker.preserveTaskReviewSummary(clear, withoutSummary({ updated_at: at(30) }), { readOrder: 50, allowMissing: true });
    const newerVersionClear = tracker.preserveTaskReviewSummary(accepted, withoutSummary({ updated_at: at(40) }));
    for (const nextClear of [newerSameFloorClear, differentlyObservedClear, newerVersionClear]) {
      expect(tracker.preserveTaskReviewSummary(nextClear, accepted)).not.toHaveProperty("review_summary");
      expect(tracker.preserveTaskReviewSummary(accepted, nextClear)).not.toHaveProperty("review_summary");
    }
    const staleClearWrite = tracker.preserveTaskReviewSummary(clear, row(3, { updated_at: at(50) }), { readOrder: 40 });
    const mergedClear = tracker.preserveTaskReviewSummary(staleClearWrite, newerSameFloorClear);
    expect(tracker.preserveTaskReviewSummary(accepted, mergedClear)).not.toHaveProperty("review_summary");
    expect(tracker.preserveTaskReviewSummary(mergedClear, accepted)).not.toHaveProperty("review_summary");
  });

  it("tracks board task rows by task_id and keeps ordinary fields independent of PR authority", () => {
    const boardRow = (version, updatedAt) => {
      const next = row(version, { task_id: task.id, updated_at: updatedAt });
      delete next.id;
      return next;
    };
    const held = tracker.preserveTaskReviewSummary(null, boardRow(7, "2026-10-08T20:20:00Z"));
    const clearWire = withoutSummary({ task_id: task.id, updated_at: "2026-10-08T20:30:00Z" });
    delete clearWire.id;
    const cleared = tracker.preserveTaskReviewSummary(held, clearWire);
    const baseRow = { ...cleared, title: "ordinary page fields" };
    const result = tracker.preserveTaskReviewSummary(cleared, boardRow(8, "2026-10-08T20:10:00Z"), { baseRow });
    expect(result).toMatchObject({ task_id: task.id, title: "ordinary page fields", review_summary: summary(8) });
  });
});

describe("atomic tracker summary writes", () => {
  it("keeps a detail summary while replacing task fields and timeline, and retaining its accepted read mark", async () => {
    const later = "tc-01K5ZQ9A1B2C3D4E5F6G7H8J9K";
    await tracker.writeTaskRecord(DEVICE, PROJECT, task.id, tracker.taskRecord(row(7, { read_through: later }), [{ id: "old" }]));
    const incoming = row(3, { title: "latest task fields", read_through: "tc-01K5ZQ8M4T0J7WQ2R6X3YB9C4F" });
    await tracker.writeTaskRecord(DEVICE, PROJECT, task.id, tracker.taskRecord(incoming, [{ id: "new" }]));
    expect(await tracker.readTaskRecord(DEVICE, PROJECT, task.id)).toMatchObject({
      task: { ...incoming, review_summary: summary(7), read_through: later }, timeline: [{ id: "new" }],
    });
  });

  it("retains detail summary absence and its authority through stale ordinary task fields", async () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const write = (taskRow) => tracker.writeTaskRecord(DEVICE, PROJECT, task.id, tracker.taskRecord(taskRow, []));
    const read = async () => (await tracker.readTaskRecord(DEVICE, PROJECT, task.id)).task;
    await write(row(7, { updated_at: at(20) }));
    await write(withoutSummary({ updated_at: at(30) }));
    await write(row(3, { title: "older ordinary fields", updated_at: at(10) }));
    expect((await read()).title).toBe("older ordinary fields");
    expect(await read()).not.toHaveProperty("review_summary");
    await write(row(3, { updated_at: at(15) }));
    expect(await read()).not.toHaveProperty("review_summary");
    await write(row(8, { updated_at: at(5) }));
    expect((await read()).review_summary.version).toBe(8);
    await write(withoutSummary({ updated_at: at(25) }));
    expect((await read()).review_summary.version).toBe(8);
    await write(withoutSummary({ updated_at: at(40) }));
    expect(await read()).not.toHaveProperty("review_summary");
  });

  for (const kind of ["whole", "query"]) {
    const write = (api, value) => kind === "whole"
      ? api.writeTasksRecord(DEVICE, PROJECT, value)
      : api.writeTasksQueryRecord(DEVICE, PROJECT, PARAMS, value);
    const read = () => kind === "whole"
      ? tracker.readTasksRecord(DEVICE, PROJECT)
      : tracker.readTasksQueryRecord(DEVICE, PROJECT, PARAMS);

    it(`preserves ${kind} list summaries under writes from two tabs`, async () => {
      await write(tracker, tracker.tasksRecord([row(3)], []));
      vi.resetModules();
      const otherTab = await import("../src/core/trackerCache.js");
      await Promise.all([
        write(tracker, tracker.tasksRecord([row(7)], [])),
        write(otherTab, tracker.tasksRecord([row(2, { title: "delayed reply" })], [])),
      ]);
      expect((await read()).tasks[0].review_summary).toEqual(summary(7));
    });

    it(`keeps ${kind} list summary over an older absence and clears it on a newer read`, async () => {
      await write(tracker, tracker.tasksRecord([row(7)], [], 20));
      await write(tracker, tracker.tasksRecord([withoutSummary()], [], 10));
      expect((await read()).tasks[0].review_summary).toEqual(summary(7));
      await write(tracker, tracker.tasksRecord([withoutSummary()], [], 15));
      expect((await read()).tasks[0].review_summary).toEqual(summary(7));
      await write(tracker, tracker.tasksRecord([withoutSummary()], [], 30));
      expect((await read()).tasks[0]).not.toHaveProperty("review_summary");
    });

    it(`does not restore a cleared ${kind} summary from a delayed older list`, async () => {
      await write(tracker, tracker.tasksRecord([row(7)], [], 20));
      await write(tracker, tracker.tasksRecord([withoutSummary()], [], 30));
      await write(tracker, tracker.tasksRecord([row(3, { title: "ordinary reply fields" })], [], 10));
      const held = await read();
      expect(held.tasks[0].title).toBe("ordinary reply fields");
      expect(held.tasks[0]).not.toHaveProperty("review_summary");
      expect(held.read_order).toBe(30);
    });

    it(`accepts a higher same-task PR version from an earlier ${kind} request`, async () => {
      await write(tracker, tracker.tasksRecord([row(7)], [], 20));
      await write(tracker, tracker.tasksRecord([row(8, { title: "ordinary reply fields" })], [], 10));
      const held = await read();
      expect(held.tasks[0]).toMatchObject({ title: "ordinary reply fields", review_summary: summary(8) });
      expect(held.read_order).toBe(20);
    });

    it(`keeps the cleared ${kind} PR floor when an older task arrives from a later request`, async () => {
      const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
      await write(tracker, tracker.tasksRecord([row(7, { updated_at: at(20) })], [], 20));
      await write(tracker, tracker.tasksRecord([withoutSummary({ updated_at: at(30) })], [], 30));
      await write(tracker, tracker.tasksRecord([row(3, { title: "ordinary later reply", updated_at: at(10) })], [], 40));
      let held = await read();
      expect(held.tasks[0].title).toBe("ordinary later reply");
      expect(held.tasks[0]).not.toHaveProperty("review_summary");
      expect(held.read_order).toBe(40);
      await write(tracker, tracker.tasksRecord([row(7, { updated_at: at(15) })], [], 50));
      expect((await read()).tasks[0]).not.toHaveProperty("review_summary");
      await write(tracker, tracker.tasksRecord([row(7, { updated_at: at(35) })], [], 60));
      expect((await read()).tasks[0].review_summary.version).toBe(7);
      await write(tracker, tracker.tasksRecord([withoutSummary({ updated_at: at(40) })], [], 70));
      await write(tracker, tracker.tasksRecord([row(8, { updated_at: at(5) })], [], 80));
      held = await read();
      expect(held.tasks[0].review_summary.version).toBe(8);
      await write(tracker, tracker.tasksRecord([row(3, { updated_at: at(50) })], [], 90));
      expect((await read()).tasks[0].review_summary.version).toBe(8);
    });
  }

  it("keeps a newer summary when a paged fold accepts an otherwise current task row", async () => {
    const address = tracker.tasksAddress(DEVICE, PROJECT);
    await tracker.writeTasksRecord(DEVICE, PROJECT, tracker.tasksRecord([row(7)], []));
    await pages.foldTasksPage(address, {
      tasks: [row(3, { title: "paged task fields" })], above: Infinity, through: -Infinity, read: 30,
    }, () => []);
    expect((await tracker.readTasksRecord(DEVICE, PROJECT)).tasks[0]).toMatchObject({
      title: "paged task fields", review_summary: summary(7),
    });
  });

  it("does not restore a cleared whole-list PR summary from late pages", async () => {
    const address = tracker.tasksAddress(DEVICE, PROJECT);
    await tracker.writeTasksRecord(DEVICE, PROJECT, tracker.tasksRecord([row(7)], [], 20));
    await tracker.writeTasksRecord(DEVICE, PROJECT, tracker.tasksRecord([withoutSummary()], [], 30));
    const fold = (read, version = 3) => pages.foldTasksPage(address, {
      tasks: [row(version, { title: "paged ordinary fields" })], above: Infinity, through: -Infinity, read,
    }, () => []);
    await fold(10);
    let held = await tracker.readTasksRecord(DEVICE, PROJECT);
    expect(held.tasks[0].title).toBe("paged ordinary fields");
    expect(held.tasks[0]).not.toHaveProperty("review_summary");
    await fold(15);
    held = await tracker.readTasksRecord(DEVICE, PROJECT);
    expect(held.tasks[0]).not.toHaveProperty("review_summary");
    expect(held.read_order).toBe(30);
    await fold(16, 8);
    expect((await tracker.readTasksRecord(DEVICE, PROJECT)).tasks[0]).not.toHaveProperty("review_summary");
  });

  it("takes only a higher PR revision from an overtaken page while keeping ordinary fields", async () => {
    const address = tracker.tasksAddress(DEVICE, PROJECT);
    await pages.foldTasksPage(address, {
      tasks: [row(7, { title: "newer ordinary fields" })], above: Infinity, through: -Infinity, read: 20,
    }, () => []);
    await pages.foldTasksPage(address, {
      tasks: [row(8, { title: "older ordinary fields" })], above: Infinity, through: -Infinity, read: 10,
    }, () => []);
    const held = await tracker.readTasksRecord(DEVICE, PROJECT);
    expect(held.tasks[0]).toMatchObject({ title: "newer ordinary fields", review_summary: summary(8) });
    expect(held.read_order).toBe(20);
  });

  it("keeps newer page membership when an older page carries a higher PR revision", async () => {
    const address = tracker.tasksAddress(DEVICE, PROJECT);
    await pages.foldTasksPage(address, { tasks: [], above: Infinity, through: -Infinity, read: 20 }, () => []);
    await pages.foldTasksPage(address, { tasks: [row(8)], above: Infinity, through: -Infinity, read: 10 }, () => []);
    expect((await tracker.readTasksRecord(DEVICE, PROJECT)).tasks).toEqual([]);
  });

  it("keeps a page's PR floor across later reads with stale ordinary timestamps", async () => {
    const at = (minute) => `2026-10-08T20:${String(minute).padStart(2, "0")}:00Z`;
    const address = tracker.tasksAddress(DEVICE, PROJECT);
    const fold = (taskRow, read) => pages.foldTasksPage(address, {
      tasks: [taskRow], above: Infinity, through: -Infinity, read,
    }, () => []);
    await tracker.writeTasksRecord(DEVICE, PROJECT, tracker.tasksRecord([row(7, { updated_at: at(20) })], [], 20));
    await tracker.writeTasksRecord(DEVICE, PROJECT, tracker.tasksRecord([withoutSummary({ updated_at: at(30) })], [], 30));
    await tracker.writeTasksRecord(DEVICE, PROJECT, tracker.tasksRecord([withoutSummary({ updated_at: at(10) })], [], 40));
    await fold(row(3, { title: "paged ordinary fields", updated_at: at(15) }), 50);
    let held = await tracker.readTasksRecord(DEVICE, PROJECT);
    expect(held.tasks[0].title).toBe("paged ordinary fields");
    expect(held.tasks[0]).not.toHaveProperty("review_summary");
    await fold(row(7, { updated_at: at(21) }), 60);
    expect((await tracker.readTasksRecord(DEVICE, PROJECT)).tasks[0]).not.toHaveProperty("review_summary");
    await fold(row(7, { updated_at: at(35) }), 70);
    expect((await tracker.readTasksRecord(DEVICE, PROJECT)).tasks[0].review_summary.version).toBe(7);
    await fold(withoutSummary({ title: "ordinary clear", updated_at: at(40) }), 80);
    await fold(row(8, { title: "stale ordinary fields", updated_at: at(25) }), 90);
    held = await tracker.readTasksRecord(DEVICE, PROJECT);
    expect(held.tasks[0]).toMatchObject({ title: "ordinary clear", review_summary: summary(8) });
    expect(held.read_order).toBe(90);
  });
});

describe("workspace list summary writes", () => {
  const address = { deviceId: DEVICE, entityId: "", kind: "workspaces" };
  const workspaceRow = (version) => ({ ...workspace, active_review: { ...workspace.active_review, version } });

  it("preserves the newest PR version on list replacement and row upsert", async () => {
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(7)]);
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(3)]);
    expect((await cache.readCached(address)).value[0].active_review.version).toBe(7);
    await sessions.upsertSessionRow(address, "workspaces", workspaceRow(2));
    expect((await cache.readCached(address)).value[0].active_review.version).toBe(7);
  });

  it("takes a new PR task in the same workspace even when its version is lower", async () => {
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(7)]);
    const next = { ...workspaceRow(1), active_review: { ...workspaceRow(1).active_review, task_id: "new-pr-task" } };
    await sessions.replaceSessionList(address, "workspaces", [next]);
    expect((await cache.readCached(address)).value[0].active_review).toEqual(next.active_review);
  });

  it("retains a PR summary that arrived after an absent list request began", async () => {
    const legacy = { ...workspace };
    delete legacy.active_review;
    await sessions.replaceSessionList(address, "workspaces", [legacy]);
    const observed = await sessions.sessionListObservation(address, "workspaces");
    await sessions.upsertSessionRow(address, "workspaces", workspaceRow(7));
    await sessions.replaceSessionList(address, "workspaces", [legacy], undefined, observed);
    expect((await cache.readCached(address)).value[0].active_review.version).toBe(7);
  });

  it("clears an absent PR summary when the read observed the held version", async () => {
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(7)]);
    const observed = await sessions.sessionListObservation(address, "workspaces");
    const legacy = { ...workspace };
    delete legacy.active_review;
    await sessions.replaceSessionList(address, "workspaces", [legacy], undefined, observed);
    expect((await cache.readCached(address)).value[0]).not.toHaveProperty("active_review");
  });

  it("does not restore a cleared summary from a delayed workspace reply", async () => {
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(7)]);
    const observed = await sessions.sessionListObservation(address, "workspaces");
    const legacy = { ...workspace };
    delete legacy.active_review;
    await sessions.replaceSessionList(address, "workspaces", [legacy], undefined, observed);
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(3)]);
    expect((await cache.readCached(address)).value[0]).not.toHaveProperty("active_review");
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(7)], undefined, observed);
    expect((await cache.readCached(address)).value[0]).not.toHaveProperty("active_review");
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(8)], undefined, observed);
    expect((await cache.readCached(address)).value[0]).not.toHaveProperty("active_review");
    const current = await sessions.sessionListObservation(address, "workspaces");
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(8)], undefined, current);
    expect((await cache.readCached(address)).value[0].active_review.version).toBe(8);
  });

  it("keeps the new PR task in a reused workspace over an old request's reply", async () => {
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(7)]);
    const observed = await sessions.sessionListObservation(address, "workspaces");
    const next = { ...workspaceRow(1), active_review: { ...workspaceRow(1).active_review, task_id: "new-pr-task" } };
    await sessions.upsertSessionRow(address, "workspaces", next);
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(7)], undefined, observed);
    expect((await cache.readCached(address)).value[0].active_review).toEqual(next.active_review);
  });

  it("accepts a higher same-PR workspace version from a request that observed an older version", async () => {
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(6)]);
    const observed = await sessions.sessionListObservation(address, "workspaces");
    await sessions.upsertSessionRow(address, "workspaces", workspaceRow(7));
    await sessions.replaceSessionList(address, "workspaces", [workspaceRow(8)], undefined, observed);
    expect((await cache.readCached(address)).value[0].active_review.version).toBe(8);
  });
});
