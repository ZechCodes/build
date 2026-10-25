// @vitest-environment jsdom
// Aggregate attribution for #858, using synthetic cache-only records.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { writeFileSync } from "node:fs";
import { IDBFactory, IDBKeyRange, IDBObjectStore, IDBDatabase } from "fake-indexeddb";
const deviceId = "follower-profile-device";
const address = (entityId, kind) => ({ deviceId, entityId, kind });
let cache, tracker, follower, onChange, projects, row, calls, transactions, gets, pending;
async function idle() {
  for (let turn = 0; turn < 3; turn += 1) {
    await vi.waitFor(() => expect(pending).toBe(0));
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  const cachedRows = await import("../src/core/cachedRows.js");
  const lineageSupport = await import("../src/core/agentLineageSupport.js");
  const rule = await import("../src/core/needsYouRule.js");
  const rosterFeed = await import("../src/core/projectRosterFeed.js");
  const lineageModel = await import("../src/core/agentLineageModel.js");
  projects = Array.from({ length: 6 }, (_, index) => ({ id: `p${index}`, projectKey: `${deviceId}|p${index}`, deviceId, entity_id: `owner${index}` }));
  for (const [kind, value] of [["projects", projects], ["workspaces", []], ["project-agents-support", { projectWorkers: true }], ["agent-lineage-support", { namesMakers: true }], ["needs-you-rule", { askedOnly: true }]]) await cache.writeCached(address("", kind), value);
  for (const project of projects) {
    const pin = { id: `pin-${project.id}`, conversation_id: `pin-conversation-${project.id}`, working: false };
    const worker = { id: `worker-${project.id}`, conversation_id: `worker-conversation-${project.id}`, working: true };
    await cache.writeCached(address(project.entity_id, "project-agents"), { entity_id: project.entity_id, pinned_agent_id: pin.id, agents: [pin, worker] });
    const task = { id: `task-${project.id}`, watched: true, status: "backlog", state: "open", updated_at: "2026-10-25T00:00:00Z", assignee: { kind: "agent", agent_id: worker.id } };
    await tracker.writeTasksRecord(deviceId, project.id, tracker.tasksRecord([task], []));
    await tracker.writeTaskRecord(deviceId, project.id, task.id, tracker.taskRecord(task, []));
  }
  row = { kind: "branch", project_id: projects[0].id, entity_id: "standalone-row", branch: "profile", agents: [] };
  await cache.writeCached(address(row.entity_id, "row"), row);
  await cachedRows.cachedFeedView(deviceId);
  calls = {
    readTasksRecord: vi.spyOn(tracker, "readTasksRecord"),
    readNeedsYouRule: vi.spyOn(rule, "readNeedsYouRule"),
    cachedFeedView: vi.spyOn(cachedRows, "cachedFeedView"),
    readAgentLineageSupport: vi.spyOn(lineageSupport, "readAgentLineageSupport"),
    projectRosterRows: vi.spyOn(rosterFeed, "projectRosterRows"),
    lineageMembers: vi.spyOn(lineageModel, "lineageMembers"),
    agentLineage: vi.spyOn(lineageModel, "agentLineage"),
  };
  pending = 0;
  const transaction = IDBDatabase.prototype.transaction;
  transactions = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    const result = transaction.apply(this, args);
    pending += 1;
    const finish = () => { pending -= 1; };
    result.addEventListener("complete", finish);
    result.addEventListener("abort", finish);
    return result;
  });
  gets = vi.spyOn(IDBObjectStore.prototype, "get");
  const { followWatchedTasks } = await import("../src/core/watchedTaskFollower.js");
  onChange = vi.fn();
  follower = followWatchedTasks({ onChange });
  follower.follow(projects);
  await vi.waitFor(() => expect(follower.entries()).toHaveLength(6));
  await idle();
});
afterEach(() => { follower?.dispose(); vi.restoreAllMocks(); });
function resetCounters() {
  Object.values(calls).forEach((call) => call.mockClear());
  gets.mockClear(); transactions.mockClear(); onChange.mockClear();
}
function counters(label) {
  const getsByKind = {};
  for (const [key] of gets.mock.calls) {
    const kind = typeof key === "string" ? decodeURIComponent(key.split("|")[2] || "unknown") : "other";
    getsByKind[kind] = (getsByKind[kind] || 0) + 1;
  }
  return { label, gets: gets.mock.calls.length, getsByKind, transactions: transactions.mock.calls.length,
    onChange: onChange.mock.calls.length, callsites: Object.fromEntries(Object.entries(calls).map(([name, spy]) => [name, spy.mock.calls.length])) };
}
it("profiles six warmed project followers on row invalidations and no-op bursts", async () => {
  const result = [];
  const before = follower.entries();
  resetCounters();
  await cache.writeCached(address(row.entity_id, "row"), { ...row, unrelatedTitle: "changed" });
  await idle(); result.push(counters("one row write, unrelated visible task state"));
  resetCounters();
  cache.announceCacheEvidence(address(row.entity_id, "row"));
  await idle(); result.push(counters("one no-op row announcement"));
  resetCounters();
  for (let index = 0; index < 32; index += 1) cache.announceCacheEvidence(address(row.entity_id, "row"));
  await idle(); result.push(counters("32 synchronous no-op row announcements"));
  resetCounters();
  for (let index = 0; index < 12; index += 1) { cache.announceCacheEvidence(address(row.entity_id, "row")); await idle(); }
  result.push(counters("12 paced no-op row announcements"));
  expect(follower.entries()).toEqual(before);
  expect(follower.runningProjectKeys().size).toBe(6);
  writeFileSync("/tmp/858-follower-unit-profile.json", JSON.stringify({ projects: 6, rows: 1, watchedTasks: 6, warm: true, scenarios: result }, null, 2));
  console.log("FOLLOWER_PROFILE", JSON.stringify(result));
});
