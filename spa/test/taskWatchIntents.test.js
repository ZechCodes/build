import { describe, expect, it } from "vitest";
import { TASK_WATCH_INTENT_CACHE, watchIntentBinding, withTaskWatchIntent, sameWatchIntent,
  reconcileTaskWatchIntent, selectTaskWatchIntentGuard } from "../src/core/taskWatchIntents.js";

const scope = { deviceId: "intent-device", projectId: "intent-project" };
const context = { storeGeneration: "store", scopeGeneration: "scope", watchGeneration: "control",
  watchPendingGeneration: "pending" };
const binding = (extra = {}) => ({ ownerId: "owner-A", taskId: "task", scope: { ...scope }, intentRead: 10,
  storeGeneration: "store", scopeGeneration: "scope", controlGeneration: "control", pendingGeneration: "pending", ...extra });
const later = (extra = {}) => binding({ ownerId: "owner-B", intentRead: 20, ...extra });
const owner = (bound = binding()) => { const { ownerId, ...rest } = bound; return { ...rest, id: ownerId }; };
const row = (bound = binding(), extra = {}) => ({ id: "task", project_id: scope.projectId,
  title: "Actual row", watched: false, read_through: "tc-20", __task_watch_intent: bound.ownerId,
  [TASK_WATCH_INTENT_CACHE]: structuredClone(bound), ...extra });
const note = (bound, disposition = "pending") => ({ latest_watch_intent: { ...bound, disposition } });
const classify = (task, latestNote, options) => reconcileTaskWatchIntent(scope, task, latestNote, context, options);
const select = (actualRows, latestNote, current = context) => selectTaskWatchIntentGuard(scope, "task", actualRows, latestNote, current);

describe("fixed watch intent bindings", () => {
  it("copies only actual owner binding fields without granting watch or payload authority", () => {
    const actualOwner = { ...owner(), watched: false, beforeWatched: true, body: "not metadata", watchRetirementEpoch: "irrelevant" };
    const bound = watchIntentBinding(actualOwner);
    expect(bound).toEqual(binding());
    expect(bound.scope).not.toBe(actualOwner.scope);
    const task = { id: "task", project_id: scope.projectId, watched: true, title: "Ordinary" };
    const stamped = withTaskWatchIntent(task, bound);
    expect(stamped).toEqual({ ...task, __task_watch_intent: "owner-A", [TASK_WATCH_INTENT_CACHE]: binding() });
    expect(stamped[TASK_WATCH_INTENT_CACHE]).not.toBe(bound);
    expect(stamped).not.toHaveProperty("__task_watch_cache");
    expect(stamped).not.toHaveProperty("__task_family_cache");
    expect(task).not.toHaveProperty("__task_watch_intent");
  });

  it.each([0, -1, NaN, Number.MAX_SAFE_INTEGER + 1])("refuses invalid intent order %s", (intentRead) => {
    expect(watchIntentBinding(owner(binding({ intentRead })))).toBeNull();
  });

  it("requires complete bindings and exact equality rather than owner ID or order alone", () => {
    expect(sameWatchIntent(binding(), structuredClone(binding()))).toBe(true);
    for (const field of ["ownerId", "taskId", "intentRead", "storeGeneration", "scopeGeneration", "controlGeneration", "pendingGeneration"]) {
      expect(sameWatchIntent(binding(), binding({ [field]: field === "intentRead" ? 11 : "different" }))).toBe(false);
      const missing = binding(); delete missing[field];
      expect(sameWatchIntent(binding(), missing)).toBe(false);
    }
    expect(sameWatchIntent(binding(), binding({ scope: { ...scope, deviceId: "different" } }))).toBe(false);
    expect(sameWatchIntent(binding(), binding({ scope: { ...scope, projectId: "different" } }))).toBe(false);
    expect(watchIntentBinding({ id: "owner-A" })).toBeNull();
  });

  it("rejects stamping a different task or project instead of manufacturing a binding", () => {
    expect(() => withTaskWatchIntent({ id: "other-task" }, binding())).toThrow(TypeError);
    expect(() => withTaskWatchIntent({ id: "task", project_id: "other-project" }, binding())).toThrow(TypeError);
    expect(() => withTaskWatchIntent({ id: "task" }, null)).toThrow(TypeError);
  });
});

describe("same-lifetime terminal and superseded intent classification", () => {
  it.each(["acknowledged", "rejected"])("retires only an exact %s intent without changing ordinary or watch paint", (disposition) => {
    const task = row(binding(), { body: "Actual body", unread_count: 7, owner_record: "still live", pending_count: 2 });
    const result = classify(task, note(binding(), disposition));
    const expected = { ...task }; delete expected.__task_watch_intent; delete expected[TASK_WATCH_INTENT_CACHE];
    expect(result).toEqual({ task: expected, retired: true, pending: null, uncertain: false });
    expect(task.__task_watch_intent).toBe("owner-A");
  });

  it("retires superseded A protection while live A remains unsettled and B pending has no actual painted row", () => {
    const task = row(binding(), { title: "A paint", owner_record: "A still live", pending_count: 2 });
    const result = classify(task, note(later()));
    expect(result).toMatchObject({ task: { watched: false, title: "A paint", owner_record: "A still live", pending_count: 2 },
      retired: true, pending: later(), uncertain: true });
    expect(result.task).not.toHaveProperty("__task_watch_intent");
    expect(result.task).not.toHaveProperty(TASK_WATCH_INTENT_CACHE);
    expect(result.task).not.toHaveProperty("__task_watch_cache");
    expect(result.task).not.toHaveProperty("__task_family_cache");
  });

  it.each(["acknowledged", "rejected"])("retires superseded A after B is %s without settling A owner or applying rollback", (disposition) => {
    const result = classify(row(binding(), { watched: false, owner_record: "A", pending_count: 1 }), note(later(), disposition));
    expect(result).toMatchObject({ task: { watched: false, owner_record: "A", pending_count: 1 },
      retired: true, pending: null, uncertain: false });
    expect(result.task).not.toHaveProperty("__task_watch_intent");
  });

  it("retains an exact pending binding and does not treat equal order as supersession", () => {
    const task = row();
    expect(classify(task, note(binding()))).toEqual({ task, retired: false, pending: binding(), uncertain: true });
    const conflict = classify(task, note(later({ intentRead: 10 })));
    expect(conflict).toMatchObject({ task, retired: false, uncertain: true });
    expect(conflict.task.__task_watch_intent).toBe("owner-A");
  });

  it.each([
    ["missing", undefined], ["older", note(later({ intentRead: 9 }), "rejected")],
    ["conflicting equal order", note(later({ intentRead: 10 }), "rejected")],
    ["missing disposition", { latest_watch_intent: binding() }],
    ["invalid disposition", note(binding(), "success")],
  ])("keeps a stamped row unknown under %s metadata", (_label, latestNote) => {
    const task = row();
    expect(classify(task, latestNote)).toMatchObject({ task, retired: false, uncertain: true });
    expect(classify(task, latestNote).task).toBe(task);
  });

  it.each(["storeGeneration", "scopeGeneration", "controlGeneration", "pendingGeneration"])(
    "refuses newer terminal and pending metadata from a different %s", (field) => {
      for (const disposition of ["pending", "acknowledged", "rejected"]) {
        const task = row();
        expect(classify(task, note(later({ [field]: "different" }), disposition))).toMatchObject({ task, retired: false, uncertain: true });
      }
    },
  );

  it("does not use a matching old-lifetime terminal when the current control lifetime changed", () => {
    const task = row();
    for (const field of ["storeGeneration", "scopeGeneration", "watchGeneration", "watchPendingGeneration"]) {
      expect(reconcileTaskWatchIntent(scope, task, note(binding(), "rejected"), { ...context, [field]: "new" }))
        .toMatchObject({ task, retired: false, uncertain: true });
    }
  });

  it("keeps bare or inconsistent stamps unresolved except for exact trusted owner settlement", () => {
    const bare = row(); delete bare[TASK_WATCH_INTENT_CACHE];
    expect(classify(bare, note(binding(), "rejected"))).toMatchObject({ task: bare, retired: false, uncertain: true });
    expect(classify(bare, undefined, { exactOwner: owner(later()) })).toMatchObject({ task: bare, retired: false, uncertain: true });
    const exact = classify(bare, undefined, { exactOwner: owner() });
    expect(exact).toMatchObject({ retired: true, pending: null, uncertain: false });
    expect(exact.task).not.toHaveProperty("__task_watch_intent");
    const inconsistent = row(binding(), { __task_watch_intent: "different-owner" });
    expect(classify(inconsistent, note(binding(), "rejected"))).toMatchObject({ task: inconsistent, retired: false, uncertain: true });
  });
});

describe("independent selection of one actual intent guard", () => {
  it("selects the actual latest pending B replica rather than a newer ordinary header or stamped A", () => {
    const ordinary = { id: "task", title: "New ordinary header", read_order: 9999, watched: true };
    const a = row(binding(), { title: "A replica", read_order: 9999 });
    const b = row(later(), { title: "B replica", watched: true, read_order: 1 });
    const result = select([ordinary, a, b], note(later()));
    expect(result).toEqual({ guard: b, pending: later(), uncertain: true });
    expect(result.guard).toBe(b);
    expect(ordinary.title).toBe("New ordinary header");
  });

  it("retains a pending B barrier without fabricating a B row or borrowing superseded A paint", () => {
    const a = row();
    expect(select([a], note(later()))).toEqual({ guard: null, pending: later(), uncertain: true });
    expect(a.__task_watch_intent).toBe("owner-A");
    expect(select([], note(later()))).toEqual({ guard: null, pending: later(), uncertain: true });
  });

  it("returns no terminal guard after retiring exact or strictly superseded metadata", () => {
    expect(select([row(), row(later())], note(later(), "rejected")))
      .toEqual({ guard: null, pending: null, uncertain: false });
  });

  it("uses one actual unresolved row conservatively without ordering conflicting owner IDs or synthesizing max paint", () => {
    const a = row(binding({ ownerId: "zzz" }), { title: "First actual", watched: false, watch_started_after: "te-90" });
    const b = row(later({ ownerId: "aaa" }), { title: "Second actual", watched: true, watch_started_after: "te-10" });
    const result = select([a, b], undefined);
    expect(result).toEqual({ guard: a, pending: null, uncertain: true });
    expect(result.guard).toBe(a);
    expect(select([b, a], undefined).guard).toBe(b);
  });

  it("does not select a foreign task/project or use cross-lifetime pending as current protection", () => {
    const foreign = row(later(), { id: "other-task" });
    const wrongProject = row(later(), { project_id: "different-project" });
    expect(select([foreign, wrongProject], note(later()))).toEqual({ guard: null, pending: later(), uncertain: true });
    const a = row();
    expect(select([a], note(later({ controlGeneration: "different" }))))
      .toEqual({ guard: a, pending: null, uncertain: true });
  });

  it("classifies no stamp and no pending as intent-only absence without inventing shared certainty", () => {
    const task = { id: "task", watched: true };
    expect(classify(task, undefined)).toEqual({ task, retired: false, pending: null, uncertain: false });
    expect(select([task], undefined)).toEqual({ guard: null, pending: null, uncertain: false });
    expect(task).not.toHaveProperty("__task_watch_cache");
  });
});
