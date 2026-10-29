import { Buffer } from "node:buffer";
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

// Independent oracle for the agent-comment fixtures below. Rust summary.rs
// checks BOTH read and watch floors, strips exactly the first prefix separator,
// and compares str as UTF-8. It accepts every string and distinguishes Some("").
// This deliberately does not import taskOpaqueMarkers or the badge's helpers.
const sourceSuffix = (id) => id.replace(/^[^-]*-/u, "");
const sourceCompare = (left, right) => Buffer.compare(Buffer.from(sourceSuffix(left)), Buffer.from(sourceSuffix(right)));
const sourceAfter = (mark, id) => mark == null || sourceCompare(id, mark) > 0;
function sourceWatch(task, timeline) {
  if (task.watch_started_after != null) return task.watch_started_after;
  return timeline.filter((entry) => entry.type === "event" && entry.kind === "watched")
    .reduce((floor, entry) => floor == null || sourceCompare(entry.id, floor) >= 0 ? entry.id : floor, null);
}
const sourceUnread = (task, timeline) => timeline.filter((entry) => entry.type === "comment"
  && entry.author.kind === "agent" && sourceAfter(task.read_through, entry.id)
  && sourceAfter(sourceWatch(task, timeline), entry.id)).length;

const stamp = "2026-10-29T12:00:00Z";
const comment = (id) => ({ type: "comment", id, author: { kind: "agent", agent_id: "author" }, body: "news", created_at: stamp });
const watched = (id) => ({ type: "event", id, kind: "watched", actor: { kind: "user" }, payload: {}, at: stamp });
const ulid = (number) => String(number).padStart(26, "0");
const watchHistory = [comment(`tc-${ulid(1)}`), watched(`te-${ulid(2)}`), comment(`tc-${ulid(3)}`)];

// Expected current and source counts are literal, independently reviewed
// witnesses. Delta assertions below make the visible behavior change explicit.
const witnesses = [
  { name: "absent watch floor uses latest watched fallback", timeline: watchHistory, legacy: 1, canonical: 1 },
  { name: "null watch floor uses latest watched fallback", watch: null, timeline: watchHistory, legacy: 1, canonical: 1 },
  { name: "present empty watch floor bypasses fallback", watch: "", timeline: watchHistory, legacy: 1, canonical: 2 },
  { name: "unknown-prefix watch floor bypasses legacy fallback", watch: "unknown-A", timeline: watchHistory, legacy: 1, canonical: 0 },
  { name: "no-hyphen watch floor bypasses legacy fallback", watch: "A", timeline: watchHistory, legacy: 1, canonical: 0 },
  { name: "multiple-hyphen watch floor bypasses legacy fallback", watch: "te-0-Z", timeline: watchHistory, legacy: 1, canonical: 2 },
  { name: "raw NUL watch floor remains present", watch: "\u0000", timeline: watchHistory, legacy: 1, canonical: 2 },
  { name: "raw whitespace watch floor remains present", watch: " ", timeline: watchHistory, legacy: 1, canonical: 2 },
  { name: "unknown-prefix read mark remains canonical", read: "unknown-B", ids: ["tc-A", "tc-C"], legacy: 2, canonical: 1 },
  { name: "unknown-prefix comment remains canonical", read: "te-B", ids: ["unknown-C"], legacy: 0, canonical: 1 },
  { name: "no-hyphen read mark remains canonical", read: "B", ids: ["tc-A", "tc-C"], legacy: 2, canonical: 1 },
  { name: "no-hyphen comment remains canonical", read: "te-B", ids: ["C"], legacy: 0, canonical: 1 },
  { name: "multiple hyphens retain the complete remainder", read: "te-A-Y", ids: ["tc-A-X", "tc-A-Z"], legacy: 0, canonical: 1 },
  { name: "NUL bytes remain significant", read: "te-A\u0000B", ids: ["tc-A\u0000A", "tc-A\u0000C"], legacy: 1, canonical: 1 },
  { name: "whitespace remains significant", read: "te- ", ids: ["tc-\t", "tc- ", "tc-  "], legacy: 1, canonical: 1 },
  { name: "UTF-16 current ordering counts high BMP after supplementary", read: "te-\u{10000}", ids: ["tc-\ue000"], legacy: 1, canonical: 0 },
  { name: "UTF-8 canonical ordering counts supplementary after high BMP", read: "te-\ue000", ids: ["tc-\u{10000}"], legacy: 0, canonical: 1 },
  { name: "Unicode is not normalized", read: "te-e\u0301", ids: ["tc-é"], legacy: 1, canonical: 1 },
  { name: "ordinary ULID read-through parity", read: `te-${ulid(2)}`, ids: [`tc-${ulid(1)}`, `tc-${ulid(3)}`], legacy: 1, canonical: 1 },
  { name: "ordinary ULID read plus watch parity", read: `tc-${ulid(3)}`, watch: `te-${ulid(2)}`, ids: [`tc-${ulid(1)}`, `tc-${ulid(4)}`], legacy: 1, canonical: 1 },
];

const scope = { deviceId: "badge-marker-oracles", projectId: "project-oracles" };
let cache, tracker, current, counts, support, project, badge;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  current = await import("../src/core/taskUnread.js");
  counts = await import("../src/core/taskAggregateReads.js");
  support = await import("../src/core/taskCountsSupport.js");
  project = await import("../src/core/projectTasksUnread.js");
});
afterEach(() => { badge?.dispose(); badge = null; });

it("pins Rust and actual #900 count witnesses rather than assuming the new JS marker model is its own oracle", () => {
  const read = (path) => readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");
  const summary = read("bridge/src/tracker/summary.rs");
  expect(summary).toContain("id.split_once('-').map_or(id, |(_, ulid)| ulid)");
  expect(summary).toContain("mark.is_none_or(|mark| when(id) > when(mark))");
  expect(summary).toContain("task.watch_started_after.as_deref().or_else(||");
  expect(summary).toContain(".filter(|entry| after(task.read_through.as_deref(), entry_id(entry)))");
  expect(summary).toContain(".filter(|entry| after(baseline, entry_id(entry)))");
  const actual = read("bridge/src/app/tests/tracker_query_exact_text.rs");
  expect(actual).toContain("fn watch_floor_missing_and_explicit_empty_remain_distinct_in_actual_counts()");
  expect(actual).toContain('[(None, 1), (Some(String::new()), 2), (None, 1)]');
  expect(actual).toContain('assert_eq!(counts["watched_unread_total"], want');
  const sql = read("bridge/src/store/task_summaries/sql.rs");
  expect(sql).toContain("substr(CAST(({value}) AS BLOB),instr(CAST(({value}) AS BLOB),x'2d')+1)");
  const activity = read("bridge/src/store/task_summaries/sql/activity.rs");
  expect(activity).toContain("c.read_ulid IS NULL OR f.ulid>c.read_ulid");
  expect(activity).toContain("COALESCE(c.watch_ulid,(SELECT MAX(ulid)");
  const fixture = JSON.parse(read("fixtures/api/v1/tasks.counts.json"));
  expect(fixture.canonical_marker_domain.watch_start_fallback).toContain("present empty watch_started_after bypasses fallback");
});

describe("actual project badge: current cache unread before canonical aggregate", () => {
  it.each(witnesses)("$name (current $legacy → canonical $canonical)", async (witness) => {
    const timeline = witness.timeline || witness.ids.map(comment);
    const task = { id: "task-oracle", project_id: scope.projectId, number: 1, title: witness.name,
      state: "open", status: "in_progress", watched: true, unread_count: 999, updated_at: stamp,
      read_through: witness.read ?? null,
      ...(Object.hasOwn(witness, "watch") ? { watch_started_after: witness.watch } : {}) };
    for (const id of [task.read_through, task.watch_started_after, ...timeline.map((entry) => entry.id)].filter((value) => value != null)) {
      expect(Buffer.byteLength(id)).toBeLessThanOrEqual(128);
    }
    expect(current.taskUnreadCount(task, { task, timeline })).toBe(witness.legacy);
    expect(sourceUnread(task, timeline)).toBe(witness.canonical);
    expect(sourceUnread(task, timeline) - current.taskUnreadCount(task, { task, timeline })).toBe(witness.canonical - witness.legacy);
    await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([task]));
    await tracker.writeTaskRecord(scope.deviceId, scope.projectId, task.id, tracker.taskRecord(task, timeline));
    badge = project.followProjectTasksUnread();
    badge.follow(scope.deviceId, scope.projectId);
    await vi.waitFor(() => expect(badge.count()).toBe(witness.legacy));

    await support.rememberTaskCountsSupport(scope.deviceId, { tasks: { counts: true } });
    const rpc = vi.fn().mockResolvedValue({ project_id: scope.projectId, unfinished_open_count: 1,
      watched_unread_total: witness.canonical, agents: [], basis: "opaque-source-basis" });
    const reader = counts.createTaskAggregateReader({ scope, callRpc: rpc, capabilities: () => ({ counts: true }), sessionOf: () => "session" });
    expect(await reader.refresh()).toBe(true);
    await vi.waitFor(() => expect(badge.count()).toBe(witness.canonical));
    expect(rpc.mock.calls).toEqual([["tasks.counts", { project_id: scope.projectId, agent_ids: [] }]]);
    expect((await reader.read()).watched_unread_total).toBe(witness.canonical);
    const detail = await tracker.readTaskRecord(scope.deviceId, scope.projectId, task.id);
    expect(current.taskUnreadCount(detail.task, detail)).toBe(witness.legacy);
    expect(detail.timeline).toEqual(timeline);
    reader.dispose();
  });
});

it("retains fetched authoritative badge data and legacy detail through unsupported-marker refusal without retry", async () => {
  const task = { id: "task-refused", project_id: scope.projectId, number: 1, watched: true, state: "open",
    status: "in_progress", updated_at: stamp, watch_started_after: "", read_through: null };
  const timeline = watchHistory;
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([task]));
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, task.id, tracker.taskRecord(task, timeline));
  badge = project.followProjectTasksUnread();
  badge.follow(scope.deviceId, scope.projectId);
  await vi.waitFor(() => expect(badge.count()).toBe(1));
  await support.rememberTaskCountsSupport(scope.deviceId, { tasks: { counts: true } });
  const fixture = JSON.parse(readFileSync(new URL("../../fixtures/api/v1/tasks.counts.json", import.meta.url), "utf8"));
  const refusal = fixture.refusals[0].reply;
  const rpc = vi.fn().mockResolvedValueOnce({ project_id: scope.projectId, unfinished_open_count: 1,
    watched_unread_total: sourceUnread(task, timeline), agents: [], basis: "fetched-basis" }).mockRejectedValueOnce(refusal);
  const reader = counts.createTaskAggregateReader({ scope, callRpc: rpc, capabilities: () => ({ counts: true }), sessionOf: () => "session" });
  await reader.refresh();
  await vi.waitFor(() => expect(badge.count()).toBe(2));
  await expect(reader.refresh()).rejects.toMatchObject({ retryable: false, details: { reason: "task_query_marker_unsupported" } });
  expect(await reader.read()).toMatchObject({ watched_unread_total: 2, basis: "fetched-basis" });
  expect(badge.count()).toBe(2);
  expect((await tracker.readTaskRecord(scope.deviceId, scope.projectId, task.id)).timeline).toEqual(timeline);
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(rpc).toHaveBeenCalledTimes(2);
  expect(badge.count()).toBe(2);
  reader.dispose();
});
