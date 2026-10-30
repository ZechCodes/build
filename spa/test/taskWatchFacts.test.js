import { describe, expect, it } from "vitest";
import {
  isTaskWatchCertain, reconcileTaskReadFacts, TASK_FAMILY_CACHE, TASK_WATCH_CACHE,
} from "../src/core/taskWatchFacts.js";

const scope = { deviceId: "watch-facts", projectId: "project" };
const control = (extra = {}) => ({ storeGeneration: "store", scopeGeneration: "scope", watchGeneration: "control",
  watchRetirementEpoch: "watch-epoch", watchPendingCount: 0, watchUncertain: false, localWatchPending: false, ...extra });
const read = (readOrder, context = control(), extra = {}) => ({ readOrder, ticket: {
  storeGeneration: context.storeGeneration, scopeGeneration: context.scopeGeneration,
  retirementEpoch: "ordinary-epoch", watchGeneration: context.watchGeneration,
  watchRetirementEpoch: context.watchRetirementEpoch, watchPending: context.watchPendingCount !== 0,
  watchUncertain: context.watchUncertain,
}, ...extra });
const row = (extra = {}) => ({ id: "task", project_id: scope.projectId, title: "Incoming ordinary title", watched: true,
  watch_started_after: "te-50", read_through: "tc-20", dismissed_through: "te-30", unread_count: 5,
  unread_mentions: 2, unread_agent_comments: 3, session_started_ms: 7, last_activity_ms: 8, ...extra });
const select = (held, incoming, readOrder, options = {}) => reconcileTaskReadFacts(scope, held, incoming,
  { context: control(), ...(readOrder === undefined ? {} : { contentRead: read(readOrder) }), ...options });
const observed = (readOrder = 3, extra = {}, options = {}) => select(undefined, row(extra), readOrder, options).task;
const family = (task, name) => task[TASK_FAMILY_CACHE]?.[name];
const ack = (contentRead, context = control(), extra = {}) => ({ ownerId: "owner", taskId: "task", intentRead: 1,
  readOrder: contentRead.readOrder, storeGeneration: context.storeGeneration, scopeGeneration: context.scopeGeneration,
  watchGeneration: context.watchGeneration, watchRetirementEpoch: context.watchRetirementEpoch, ...extra });

describe("per-row watch observation selection", () => {
  it("records a genuine selected source without borrowing ordinary, receipt or timestamp authority", () => {
    const result = select(undefined, row({ updated_at: "1900-01-01", read_order: 9999 }), 3);
    expect(result).toMatchObject({ acceptedWatch: true, watchCertain: true, selectedWatchRead: 3, needsUnreadRefresh: false });
    expect(result.task[TASK_WATCH_CACHE].observed_read_order).toBe(3);
    for (const name of ["total", "attention", "activity"]) expect(family(result.task, name).observed_read_order).toBe(3);
    expect(isTaskWatchCertain(result.task, control())).toBe(true);
  });

  it("retains newer genuine watch, counters and activity while preserving the caller's incoming ordinary and PR fields", () => {
    const held = observed(3);
    const result = select(held, row({ watched: false, watch_started_after: "te-10", read_through: "tc-10",
      dismissed_through: "te-10", unread_count: 99, session_started_ms: 1, last_activity_ms: 2,
      updated_at: "9999-01-01", review_summary: { version: 2 } }), 2);
    expect(result).toMatchObject({ acceptedWatch: false, watchCertain: true, selectedWatchRead: 3 });
    expect(result.task).toMatchObject({ watched: true, watch_started_after: "te-50", read_through: "tc-20",
      dismissed_through: "te-30", unread_count: 5, session_started_ms: 7, last_activity_ms: 8,
      title: "Incoming ordinary title", review_summary: { version: 2 } });
  });

  it("advances an older source's read marker without restamping watch, still-covered total or activity", () => {
    const result = select(observed(3), row({ read_through: "tc-30", unread_count: 99, unread_mentions: 99,
      unread_agent_comments: 99, session_started_ms: 1, last_activity_ms: 2 }), 2);
    expect(result.task).toMatchObject({ watched: true, watch_started_after: "te-50", read_through: "tc-30",
      unread_count: 5, session_started_ms: 7, last_activity_ms: 8 });
    expect(result.task).not.toHaveProperty("unread_mentions");
    expect(result.task).not.toHaveProperty("unread_agent_comments");
    expect(family(result.task, "attention")).toBeUndefined();
    for (const name of ["total", "activity"]) expect(family(result.task, name).observed_read_order).toBe(3);
    expect(result.task[TASK_WATCH_CACHE].observed_read_order).toBe(3);
    expect(result).toMatchObject({ watchCertain: true, needsUnreadRefresh: true, selectedWatchRead: 3 });
  });

  it("makes overtaken total and attention unknown while retaining certain watch and independently paired activity", () => {
    const result = select(observed(3), row({ read_through: "tc-60", unread_count: 99 }), 2);
    for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) expect(result.task).not.toHaveProperty(field);
    expect(family(result.task, "total")).toBeUndefined();
    expect(family(result.task, "attention")).toBeUndefined();
    expect(result.task).toMatchObject({ read_through: "tc-60", session_started_ms: 7, last_activity_ms: 8 });
    expect(result).toMatchObject({ watchCertain: true, needsUnreadRefresh: true, selectedWatchRead: 3 });
  });

  it("allows an honest later same-floor decrease without count arithmetic", () => {
    const result = select(observed(3), row({ unread_count: 1, unread_mentions: 0, unread_agent_comments: 1 }), 4);
    expect(result.task).toMatchObject({ unread_count: 1, unread_mentions: 0, unread_agent_comments: 1 });
    expect(family(result.task, "total").observed_read_order).toBe(4);
    expect(family(result.task, "attention").observed_read_order).toBe(4);
  });

  it.each([null, "", "\0", " ", "te-10", "unknown-a-b", "te-\u{10000}"])(
    "preserves a genuine later raw watch incarnation %j independently of monotonic markers", (floor) => {
      const result = select(observed(3), row({ watch_started_after: floor, read_through: "tc-10", dismissed_through: "te-10" }), 4);
      expect(result.task.watch_started_after).toBe(floor);
      expect(result.task.read_through).toBe("tc-20");
      expect(result.task.dismissed_through).toBe("te-30");
      expect(result).toMatchObject({ acceptedWatch: true, watchCertain: true, selectedWatchRead: 4 });
    },
  );

  it("distinguishes an omitted watch floor from present null and empty instead of normalizing lineage", () => {
    const incoming = row(); delete incoming.watch_started_after;
    const omitted = select(observed(), incoming, 4);
    expect(omitted.task).not.toHaveProperty("watch_started_after");
    expect(omitted.watchCertain).toBe(true);
    const present = select(omitted.task, row({ watch_started_after: null }), 5);
    expect(present.task).toHaveProperty("watch_started_after", null);
    expect(present.task[TASK_WATCH_CACHE].observed_read_order).toBe(5);
  });

  it("keeps watch certainty independent from optional missing counter values", () => {
    const incoming = row();
    for (const key of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete incoming[key];
    const result = select(undefined, incoming, 3);
    expect(result).toMatchObject({ watchCertain: true, needsUnreadRefresh: true });
    for (const key of ["unread_count", "unread_mentions", "unread_agent_comments"]) expect(result.task).not.toHaveProperty(key);
  });

  it("preserves the existing unwatch clearing policy without introducing activity or fabricated zero counters", () => {
    const result = select(observed(), row({ watched: false }), 4);
    for (const key of ["watch_started_after", "unread_count", "unread_mentions", "unread_agent_comments", "session_started_ms", "last_activity_ms"]) {
      expect(result.task).not.toHaveProperty(key);
    }
    expect(result.task[TASK_FAMILY_CACHE]).toBeUndefined();
    expect(result).toMatchObject({ watchCertain: true, needsUnreadRefresh: false, selectedWatchRead: 4 });
  });

  it("never manufactures current proof from raw rows or higher copy/list/receipt clocks", () => {
    const raw = select(undefined, row({ read_order: 10000, watch_read_order: 10000, updated_at: "9999-01-01" }));
    expect(raw.watchCertain).toBe(false);
    expect(raw.selectedWatchRead).toBeNull();
    expect(raw.task[TASK_WATCH_CACHE]).toBeUndefined();
    const held = observed(3), copied = { ...held, read_order: 99999, watch_read_order: 99999, title: "Copied ordinary bytes" };
    const result = select(held, copied);
    expect(result.task[TASK_WATCH_CACHE].observed_read_order).toBe(3);
    expect(result).toMatchObject({ watchCertain: true, selectedWatchRead: 3, acceptedWatch: false });
    const changed = select(held, { ...copied, watched: false });
    expect(changed.task.watched).toBe(true);
    expect(changed.selectedWatchRead).toBe(3);
  });

  it("does not let equal snapshots lower a newer source or transfer proof across task identities", () => {
    const older = observed(2), held = observed(3);
    expect(select(held, older).selectedWatchRead).toBe(3);
    const changed = select(undefined, { ...held, id: "different-task" });
    expect(changed.watchCertain).toBe(false);
    expect(changed.selectedWatchRead).toBeNull();
  });

  it("does not borrow another task's markers or a foreign project's observation proof", () => {
    const result = select(observed(), row({ id: "other-task", read_through: null, dismissed_through: null }), 4);
    expect(result.task).toMatchObject({ id: "other-task", read_through: null, dismissed_through: null });
    expect(select(undefined, row({ project_id: "foreign" }), 4).watchCertain).toBe(false);
  });

  it("keeps watch proof on a marker-only acknowledgment while retiring only overtaken counter families", () => {
    const result = select(observed(), { id: "task", read_through: "tc-60" });
    expect(result.task).toMatchObject({ watched: true, watch_started_after: "te-50", read_through: "tc-60",
      session_started_ms: 7, last_activity_ms: 8 });
    expect(result.task).not.toHaveProperty("unread_count");
    expect(result.task).not.toHaveProperty("unread_mentions");
    expect(result).toMatchObject({ selectedWatchRead: 3, watchCertain: true, acceptedWatch: false });
  });

  it("uses an actual normalized watch guard as an independent source without copying its ordinary fields or markers wholesale", () => {
    const guard = observed(5, { title: "Guard title", read_through: "tc-90", unread_count: 2 });
    const result = select(observed(3), row({ title: "Incoming title", read_through: "tc-20", review_summary: { version: 9 } }), 2,
      { watchGuard: guard });
    expect(result.task).toMatchObject({ title: "Incoming title", read_through: "tc-20", review_summary: { version: 9 }, watched: true });
    expect(result.selectedWatchRead).toBe(5);
    expect(result.watchCertain).toBe(true);
    // Newest genuine family source wins before pairing. Incompatibility makes
    // its counters unknown; it cannot resurrect older compatible source3.
    for (const key of ["unread_count", "unread_mentions", "unread_agent_comments"]) expect(result.task).not.toHaveProperty(key);
    expect(result.task[TASK_FAMILY_CACHE].unknown.attention.observed_read_order).toBe(5);
  });

  it("keeps an incompatible newest family unknown across later older and equal-source copies, then allows genuine newer recovery", () => {
    const guard = observed(5, { read_through: "tc-90" });
    const initial = select(observed(3), row(), 2, { watchGuard: guard });
    for (const copied of [observed(3), observed(5)]) {
      const next = select(initial.task, copied);
      expect(next.task).not.toHaveProperty("unread_count");
      expect(next.task[TASK_FAMILY_CACHE].unknown.total.observed_read_order).toBe(5);
    }
    const recovered = select(initial.task, row({ unread_count: 1 }), 6);
    expect(recovered.task.unread_count).toBe(1);
    expect(family(recovered.task, "total").observed_read_order).toBe(6);
    expect(recovered.task[TASK_FAMILY_CACHE].unknown).toBeUndefined();
  });

  it("does not resurrect old known values when a copied unknown barrier has a retired control epoch", () => {
    const initial = select(observed(3), row(), 2, { watchGuard: observed(5, { read_through: "tc-90" }) });
    const metadata = structuredClone(initial.task[TASK_FAMILY_CACHE]);
    metadata.unknown.total.watchRetirementEpoch = "retired-control";
    const next = select({ ...initial.task, [TASK_FAMILY_CACHE]: metadata }, observed(3));
    expect(next.task).not.toHaveProperty("unread_count");
    expect(next.watchCertain).toBe(true);
  });

  it("does not mint a family observation or unknown barrier from unsupported counter coverage", () => {
    const incoming = row(); delete incoming.unread_mentions; delete incoming.unread_agent_comments;
    const result = select(observed(3), incoming, 4);
    expect(result.task).toMatchObject({ unread_mentions: 2, unread_agent_comments: 3 });
    expect(family(result.task, "attention").observed_read_order).toBe(3);
    expect(result.task[TASK_FAMILY_CACHE].unknown?.attention).toBeUndefined();
    expect(result).toMatchObject({ watchCertain: true, needsUnreadRefresh: true });
  });

  it("retains honest source-ordered uncertain paint without promoting copies into known family proof", () => {
    const context = control({ watchUncertain: true });
    const first = select(undefined, row(), 3, { context, contentRead: read(3, context) });
    expect(first.task).toMatchObject({ unread_count: 5, unread_mentions: 2, unread_agent_comments: 3,
      session_started_ms: 7, last_activity_ms: 8 });
    expect(first).toMatchObject({ watchCertain: false, needsUnreadRefresh: true, selectedWatchRead: 3 });
    for (const name of ["total", "attention", "activity"]) {
      expect(family(first.task, name)).toBeUndefined();
      expect(first.task[TASK_FAMILY_CACHE].paint[name].observed_read_order).toBe(3);
    }
    const older = select(first.task, row({ unread_count: 99, unread_mentions: 99, unread_agent_comments: 99,
      session_started_ms: 99, last_activity_ms: 99 }), 2, { context, contentRead: read(2, context) });
    expect(older.task).toMatchObject({ unread_count: 5, unread_mentions: 2, session_started_ms: 7, last_activity_ms: 8 });
    const copied = select(undefined, older.task);
    expect(copied).toMatchObject({ watchCertain: false, needsUnreadRefresh: true });
    expect(family(copied.task, "total")).toBeUndefined();
    expect(copied.task[TASK_FAMILY_CACHE].paint.total.observed_read_order).toBe(3);
    const recovered = select(copied.task, row({ unread_count: 1 }), 4);
    expect(recovered).toMatchObject({ watchCertain: true, needsUnreadRefresh: false });
    expect(family(recovered.task, "total").observed_read_order).toBe(4);
    expect(recovered.task[TASK_FAMILY_CACHE].paint).toBeUndefined();
  });

  it("retires a previous watch incarnation's unknown barriers on a genuine new incarnation", () => {
    const initial = select(observed(3), row(), 2, { watchGuard: observed(5, { read_through: "tc-90" }) });
    const incoming = row({ watch_started_after: "" }); delete incoming.unread_mentions; delete incoming.unread_agent_comments;
    const result = select(initial.task, incoming, 6);
    expect(result.task.watch_started_after).toBe("");
    expect(result.task[TASK_FAMILY_CACHE].unknown).toBeUndefined();
    expect(result.task).not.toHaveProperty("unread_mentions");
  });
});

describe("independent watch admission and owner acknowledgment", () => {
  it.each([
    ["missing control", {}], ["pending", control({ watchPendingCount: 1 })],
    ["orphan/uncertain", control({ watchUncertain: true })], ["missing count", control({ watchPendingCount: null })],
    ["local pending", control({ localWatchPending: true })],
  ])("does not certify watch facts under %s", (_label, context) => {
    const result = select(observed(), row({ watched: false }), 4, { context });
    expect(result.watchCertain).toBe(false);
    expect(isTaskWatchCertain(result.task, context)).toBe(false);
    expect(isTaskWatchCertain(result.task, control())).toBe(false);
  });

  it("blocks post-optimism/pre-ack wire paint while retaining stronger monotonic markers", () => {
    const context = control({ watchPendingCount: 1 });
    const result = select(observed(), row({ watched: false, watch_started_after: "te-90", read_through: "tc-60",
      unread_count: 999, session_started_ms: 999 }), undefined, { context, contentRead: read(4, context) });
    expect(result.task).toMatchObject({ watched: true, watch_started_after: "te-50", read_through: "tc-60", session_started_ms: 7 });
    expect(result.task).not.toHaveProperty("unread_count");
    expect(result).toMatchObject({ acceptedWatch: false, watchCertain: false, needsUnreadRefresh: true });
  });

  it.each(["storeGeneration", "scopeGeneration", "watchGeneration", "watchRetirementEpoch"])(
    "refuses a mismatched captured %s without borrowing ordinary retirement authority", (key) => {
      const contentRead = read(4); contentRead.ticket[key] = "obsolete";
      const result = select(undefined, row(), undefined, { contentRead });
      expect(result.watchCertain).toBe(false);
      expect(result.acceptedWatch).toBe(false);
    },
  );

  it("does not use the ordinary retirement epoch to reject an otherwise valid watch observation", () => {
    const contentRead = read(4); contentRead.ticket.retirementEpoch = "different-ordinary-epoch";
    expect(select(undefined, row(), undefined, { contentRead }).watchCertain).toBe(true);
  });

  it("admits only a matching owner's genuine acknowledgment across the settlement watch epoch", () => {
    const before = control({ watchPendingCount: 1, localWatchPending: true }), contentRead = read(4, before);
    const after = control({ watchRetirementEpoch: "settlement", watchPendingCount: 1, localWatchPending: true });
    const acknowledgment = ack(contentRead, after);
    const result = select(observed(), row({ watched: false }), undefined, { context: after, contentRead, acknowledgment });
    expect(result).toMatchObject({ acceptedWatch: true, watchCertain: true, selectedWatchRead: 4 });
    expect(result.task.watched).toBe(false);
    expect(isTaskWatchCertain(result.task, after)).toBe(true);
    expect(isTaskWatchCertain(result.task, { ...after, watchRetirementEpoch: "later" })).toBe(false);
  });

  it.each(["taskId", "readOrder", "storeGeneration", "scopeGeneration", "watchGeneration", "watchRetirementEpoch"])(
    "does not treat a mismatched acknowledgment %s as an owned settlement", (key) => {
      const context = control({ watchPendingCount: 1 }), contentRead = read(4, context);
      const acknowledgment = ack(contentRead, context, { [key]: key === "readOrder" ? 99 : "other" });
      expect(select(observed(), row({ watched: false }), undefined, { context, contentRead, acknowledgment }).acceptedWatch).toBe(false);
    },
  );

  it("cannot bootstrap sticky unknown from its own matching acknowledgment or a copied proof", () => {
    const context = control({ watchUncertain: true }), contentRead = read(4, context);
    const result = select(undefined, row(), undefined, { context, contentRead, acknowledgment: ack(contentRead, context) });
    expect(result.watchCertain).toBe(false);
    expect(isTaskWatchCertain(result.task, context)).toBe(false);
    expect(select(undefined, result.task, undefined, { context }).watchCertain).toBe(false);
  });
});

describe("unchanged canonical and legacy marker policies", () => {
  it("uses each existing policy for counter pairing without treating semantic floor equality as freshness", () => {
    for (const markerPolicy of ["canonical", "legacy"]) {
      const held = observed(3, { read_through: "tc-\u{10000}" }, { markerPolicy });
      const result = select(held, row({ read_through: "te-\ue000", unread_count: 99 }), 2, { markerPolicy });
      expect(result.selectedWatchRead).toBe(3);
      if (markerPolicy === "canonical") {
        expect(result.task.unread_count).toBe(5);
        expect(family(result.task, "total").observed_read_order).toBe(3);
      } else {
        expect(result.task).not.toHaveProperty("unread_count");
        expect(family(result.task, "total")).toBeUndefined();
      }
    }
  });

  it.each([
    ["unknown-A-B", "unknown-A-C", "unknown-A-C", "unknown-A-C"],
    ["tc-\u{10000}", "te-\ue000", "tc-\u{10000}", "te-\ue000"],
    ["te-50", "", "te-50", "te-50"],
    [null, "", "", ""], [null, "\0", "\0", "\0"], [null, " ", " ", " "],
  ])("keeps separate raw-floor results for %j / %j", (heldMark, incomingMark, canonical, legacy) => {
    for (const [markerPolicy, expected] of [["canonical", canonical], ["legacy", legacy]]) {
      const held = observed(3, { read_through: heldMark }, { markerPolicy });
      const result = select(held, row({ read_through: incomingMark }), 2, { markerPolicy });
      expect(result.task.read_through).toBe(expected);
      expect(result.task[TASK_WATCH_CACHE].observed_read_order).toBe(3);
    }
  });
});
