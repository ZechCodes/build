/** @vitest-environment jsdom */
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, tracker, follow, contexts, follower;
const deviceId = "review-loop-offline";
const project = { id: "project", deviceId, projectKey: `${deviceId}|project`, name: "Offline reviews" };
const address = (kind, entityId = "", sub) => ({ deviceId, entityId, kind, ...(sub ? { sub } : {}) });
const row = (id, fields = {}) => ({ id, title: id, watched: true, state: "open", status: "in_review", assignee: { kind: "user" }, ...fields });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  contexts = await import("../src/core/deviceContexts.js");
  ({ followWatchedTasks: follow } = await import("../src/core/watchedTaskFollower.js"));
});
afterEach(() => { follower?.dispose(); contexts.resetDeviceContexts(); });

it("paints cached Legacy and Rejected inbox facts offline, with Done service attention until read", async () => {
  const support = { reject: true, verdictDelivery: true };
  await cache.writeCached(address("task-review-support"), support);
  await cache.writeCached(address("needs-you-rule"), { askedOnly: true });
  await cache.writeCached(address("task-review", project.id, "legacy"), { review: { task_id: "legacy", snapshots: [] } });
  const rejected = row("rejected", { status: "done", unread_service_mentions: 1, review_summary: { status: "rejected" } });
  const legacy = row("legacy");
  await cache.writeCached(tracker.tasksAddress(deviceId, project.id), { tasks: [rejected, legacy] });
  follower = follow({ onChange: () => {} });
  follower.follow([project]);
  const { inboxRowHtml } = await import("../src/core/inbox.js");
  await vi.waitFor(() => {
    const entries = follower.entries();
    expect(entries).toHaveLength(2);
    expect(entries.map(inboxRowHtml).join("")).toContain("Legacy review");
    expect(entries.map(inboxRowHtml).join("")).toContain("Rejected");
    expect(entries.find(({ taskId }) => taskId === "rejected")).toMatchObject({ quiet: false, facts: "Mentioned you" });
  });
  expect(contexts.contextFor(deviceId)).toBeNull();
  await cache.writeCached(tracker.tasksAddress(deviceId, project.id), { tasks: [{ ...rejected, unread_service_mentions: 0 }, legacy] });
  await vi.waitFor(() => expect(follower.entries().map(({ taskId }) => taskId)).toEqual(["legacy"]));
});

it("uses cached marked notices when a finished review's older summary has no service count", async () => {
  await cache.writeCached(address("task-review-support"), { reject: true, verdictDelivery: true });
  await cache.writeCached(address("needs-you-rule"), { askedOnly: true });
  const value = row("older-rejected", { status: "done", review_summary: { status: "rejected" } });
  const comment = { id: "tc-02", type: "comment", author: { kind: "build" }, review_notice: "verdict_undelivered", mentions_user: true };
  await cache.writeCached(tracker.tasksAddress(deviceId, project.id), { tasks: [value] });
  await cache.writeCached(tracker.taskAddress(deviceId, project.id, value.id), { task: value, timeline: [comment] });
  follower = follow({ onChange: () => {} });
  follower.follow([project]);
  await vi.waitFor(() => expect(follower.entries()[0]).toMatchObject({ taskId: value.id, quiet: false }));
  await cache.writeCached(tracker.taskAddress(deviceId, project.id, value.id), { task: { ...value, read_through: "tc-02" }, timeline: [comment] });
  await vi.waitFor(() => expect(follower.entries()).toEqual([]));
});

it("cannot restore Legacy metadata from an older read after cached capability revocation", async () => {
  await cache.writeCached(address("task-review-support"), { reject: true, verdictDelivery: true });
  await cache.writeCached(address("needs-you-rule"), { askedOnly: true });
  await cache.writeCached(tracker.tasksAddress(deviceId, project.id), { tasks: [row("legacy")] });
  const facts = await import("../src/core/taskReviewRowFacts.js");
  const original = facts.readReviewRowFacts;
  let release;
  const delayed = vi.spyOn(facts, "readReviewRowFacts").mockImplementationOnce(() => new Promise((resolve) => { release = resolve; }));
  follower = follow({ onChange: () => {} });
  follower.follow([project]);
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  await cache.writeCached(address("task-review-support"), { reject: false, verdictDelivery: false });
  await vi.waitFor(() => expect(follower.entries()[0]?.reviewSupport?.reject).toBe(false));
  release(new Map([["legacy", { mode: "snapshot" }]]));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(follower.entries()[0].reviewSummary).toBeNull();
  delayed.mockImplementation(original);
  delayed.mockRestore();
});
