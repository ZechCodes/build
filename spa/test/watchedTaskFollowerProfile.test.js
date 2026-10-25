// @vitest-environment jsdom
// Aggregate attribution for #858, using synthetic cache-only records.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { writeFileSync } from "node:fs";
import { IDBFactory, IDBKeyRange, IDBObjectStore, IDBDatabase } from "fake-indexeddb";
const deviceId = "follower-profile-device";
const address = (entityId, kind) => ({ deviceId, entityId, kind });
let cache, tracker, follower, onChange, projects, row, calls, transactions, gets, pending, contexts;
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
  contexts = await import("../src/core/deviceContexts.js");
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
afterEach(() => { follower?.dispose(); contexts.resetDeviceContexts(); vi.restoreAllMocks(); });
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

it("keeps row-only no-op hints out of task and rule dependencies", async () => {
  resetCounters();
  for (let index = 0; index < 32; index += 1) cache.announceCacheEvidence(address(row.entity_id, "row"));
  await idle();
  expect(calls.readTasksRecord).not.toHaveBeenCalled();
  expect(calls.readNeedsYouRule).not.toHaveBeenCalled();
  expect(calls.readAgentLineageSupport).not.toHaveBeenCalled();
  expect(calls.cachedFeedView.mock.calls.length).toBeLessThanOrEqual(2);
  expect(calls.lineageMembers.mock.calls.length).toBeLessThanOrEqual(12);
  expect(onChange).not.toHaveBeenCalled();
});

it("publishes finite agent passes under continued hints and lets actions commit while held", async () => {
  const task = { id: "task-user", project_id: projects[0].id, title: "Assigned immediately", status: "ready",
    assignee: { kind: "agent", agent_id: "worker-p0" }, updated_at: "2026-10-25T00:00:00Z" };
  const actionRpc = vi.fn(async () => ({ task, timeline: [] }));
  const context = contexts.adoptDeviceSession({ deviceId, call: actionRpc, close() {}, peer() {}, onCarrier() {} });
  contexts.adoptBridgeSelection(context, { major: 2, version: "2.0.0" }, {});
  follower.follow(projects);
  await idle();
  resetCounters();
  const readMany = cache.readCachedMany;
  const releases = [];
  const heldReads = vi.spyOn(cache, "readCachedMany").mockImplementation(async (addresses) => {
    const result = await readMany(addresses);
    if (!addresses.every(({ kind }) => kind === "row" || kind === "project-agents")) return result;
    await new Promise((resolve) => releases.push(resolve));
    return result;
  });
  const rosterAddress = address(projects[0].entity_id, "project-agents");
  const roster = (await cache.readCached(rosterAddress)).value;
  await cache.writeCached(rosterAddress, { ...roster, agents: roster.agents.map((agent) => ({ ...agent, working: false })) });
  await vi.waitFor(() => expect(releases).toHaveLength(1));
  cache.announceCacheEvidence(address(row.entity_id, "row"));
  const { createTaskWaitingOnActions } = await import("../src/core/taskWaitingOnActions.js");
  const actions = createTaskWaitingOnActions({ deviceId, projectId: projects[0].id, taskId: task.id, call: context.rpc });
  await actions.assign({ assignee: task.assignee });
  expect(actionRpc).toHaveBeenCalledExactlyOnceWith("tasks.assign", { task_id: task.id, assignee: task.assignee });
  expect((await cache.readCached(tracker.taskAddress(deviceId, projects[0].id, task.id)))?.value.task).toEqual(task);
  expect(releases).toHaveLength(1);
  releases[0]();
  try {
    await vi.waitFor(() => expect(follower.runningProjectKeys().size).toBe(5));
    await vi.waitFor(() => expect(releases).toHaveLength(2));
    cache.announceCacheEvidence(address(row.entity_id, "row"));
  } finally {
    follower.dispose();
    releases.forEach((release) => release());
    await idle();
    expect(heldReads).toHaveBeenCalledTimes(2);
  }
});

it("updates only the task or policy dependency that changed and clears prefix evictions", async () => {
  resetCounters();
  const record = await tracker.readTasksRecord(deviceId, projects[0].id);
  calls.readTasksRecord.mockClear();
  await tracker.writeTasksRecord(deviceId, projects[0].id, { ...record, tasks: record.tasks.map((task) => ({ ...task, title: "New task title" })) });
  await idle();
  expect(follower.entries().some((entry) => entry.title === "New task title")).toBe(true);
  expect(calls.readTasksRecord).toHaveBeenCalledTimes(1);
  expect(calls.readNeedsYouRule).not.toHaveBeenCalled();
  expect(calls.lineageMembers).not.toHaveBeenCalled();
  resetCounters();
  await cache.writeCached(address("", "needs-you-rule"), { askedOnly: false });
  await idle();
  expect(calls.readNeedsYouRule).toHaveBeenCalledTimes(1);
  expect(calls.readTasksRecord).not.toHaveBeenCalled();
  expect(calls.lineageMembers).not.toHaveBeenCalled();
  await cache.evictEntity(deviceId, projects[0].id);
  await idle();
  expect(follower.entries()).toHaveLength(5);
  await cache.wipeCache();
  await idle();
  expect(follower.entries()).toEqual([]);
  expect(follower.runningProjectKeys().size).toBe(0);
});

it("discards held task responses after prefix eviction and disposed sources", async () => {
  const readTasks = tracker.readTasksRecord;
  let release;
  vi.spyOn(tracker, "readTasksRecord").mockImplementationOnce(async (...args) => {
    const record = await readTasks(...args);
    await new Promise((resolve) => { release = resolve; });
    return record;
  });
  cache.announceCacheEvidence(tracker.tasksAddress(deviceId, projects[0].id));
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  await cache.evictEntity(deviceId, projects[0].id);
  release();
  await idle();
  expect(follower.entries()).toHaveLength(5);
  resetCounters();
  follower.follow(projects.slice(1));
  await cache.writeCached(address(projects[0].entity_id, "project-agents"), { entity_id: projects[0].entity_id, pinned_agent_id: "replacement", agents: [{ id: "replacement", working: true }] });
  await idle();
  expect(onChange).not.toHaveBeenCalled();
  follower.dispose();
  resetCounters();
  cache.announceCacheEvidence(address(row.entity_id, "row"));
  await idle();
  expect(onChange).not.toHaveBeenCalled();
  expect(calls.lineageMembers).not.toHaveBeenCalled();
});

it("keeps the replacement session current when an older task read finishes late", async () => {
  const readTasks = tracker.readTasksRecord;
  let release;
  vi.spyOn(tracker, "readTasksRecord").mockImplementationOnce(async (...args) => {
    const record = await readTasks(...args);
    await new Promise((resolve) => { release = resolve; });
    return record;
  });
  cache.announceCacheEvidence(tracker.tasksAddress(deviceId, projects[0].id));
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  const record = await readTasks(deviceId, projects[0].id);
  await tracker.writeTasksRecord(deviceId, projects[0].id, { ...record, tasks: record.tasks.map((task) => ({ ...task, title: "Replacement session title" })) });
  contexts.adoptDeviceSession({ deviceId, call: vi.fn(), close() {}, peer() {}, onCarrier() {} });
  follower.follow(projects);
  await vi.waitFor(() => expect(follower.entries().find((entry) => entry.projectId === projects[0].id)?.title).toBe("Replacement session title"));
  release();
  await idle();
  expect(follower.entries().find((entry) => entry.projectId === projects[0].id).title).toBe("Replacement session title");
});

it.each([true, false])("waits for actual cold policy %s before publishing task attention", async (askedOnly) => {
  follower.dispose();
  await cache.writeCached(address("", "needs-you-rule"), { askedOnly });
  const record = await tracker.readTasksRecord(deviceId, projects[0].id);
  await tracker.writeTasksRecord(deviceId, projects[0].id, { ...record, tasks: record.tasks.map((task) => ({ ...task, status: "in_review" })) });
  const rule = await import("../src/core/needsYouRule.js");
  const readRule = rule.readNeedsYouRule;
  let release;
  vi.spyOn(rule, "readNeedsYouRule").mockImplementationOnce(async (...args) => {
    const value = await readRule(...args);
    await new Promise((resolve) => { release = resolve; });
    return value;
  });
  const snapshots = [];
  onChange = vi.fn(() => snapshots.push(follower.entries().map(({ quiet, reason }) => ({ quiet, reason }))));
  const { followWatchedTasks } = await import("../src/core/watchedTaskFollower.js");
  follower = followWatchedTasks({ onChange });
  follower.follow(projects.slice(0, 1));
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  await idle();
  try {
    expect(onChange).not.toHaveBeenCalled();
  } finally { release(); }
  await vi.waitFor(() => expect(follower.entries()).toHaveLength(1));
  expect(snapshots.length).toBeGreaterThan(0);
  expect(snapshots.flat().every((entry) => entry.quiet === askedOnly)).toBe(true);
  expect(snapshots.flat().every((entry) => entry.reason.includes("In review") === !askedOnly)).toBe(true);
});

it("does not discard a held policy update after an unrelated entity eviction", async () => {
  await cache.writeCached(address("", "needs-you-rule"), { askedOnly: false });
  const record = await tracker.readTasksRecord(deviceId, projects[0].id);
  await tracker.writeTasksRecord(deviceId, projects[0].id, { ...record, tasks: record.tasks.map((task) => ({ ...task, status: "in_review" })) });
  await idle();
  expect(follower.entries().find((entry) => entry.projectId === projects[0].id).quiet).toBe(false);
  const rule = await import("../src/core/needsYouRule.js");
  const readRule = rule.readNeedsYouRule;
  let release;
  vi.spyOn(rule, "readNeedsYouRule").mockImplementationOnce(async (...args) => {
    const value = await readRule(...args);
    await new Promise((resolve) => { release = resolve; });
    return value;
  });
  await cache.writeCached(address("", "needs-you-rule"), { askedOnly: true });
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  await cache.evictEntity(deviceId, row.entity_id);
  release();
  await idle();
  expect(follower.entries().find((entry) => entry.projectId === projects[0].id).quiet).toBe(true);
});

it("uses the actual cached policy for a project joining a warm device", async () => {
  follower.follow(projects.slice(0, 1));
  await idle();
  const record = await tracker.readTasksRecord(deviceId, projects[1].id);
  await tracker.writeTasksRecord(deviceId, projects[1].id, { ...record, tasks: record.tasks.map((task) => ({ ...task, status: "in_review" })) });
  const readMany = cache.readCachedMany;
  let release;
  vi.spyOn(cache, "readCachedMany").mockImplementationOnce(async (addresses) => {
    const records = await readMany(addresses);
    await new Promise((resolve) => { release = resolve; });
    return records;
  });
  cache.announceCacheEvidence(address(row.entity_id, "row"));
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  const snapshots = [];
  onChange.mockImplementation(() => {
    const entry = follower.entries().find((item) => item.projectId === projects[1].id);
    if (entry) snapshots.push({ quiet: entry.quiet, reason: entry.reason });
  });
  follower.follow(projects.slice(0, 2));
  await idle();
  try { expect(snapshots).toEqual([]); }
  finally { release(); }
  await vi.waitFor(() => expect(snapshots.length).toBeGreaterThan(0));
  await idle();
  expect(snapshots.length).toBeGreaterThan(0);
  expect(snapshots.every((entry) => entry.quiet && !entry.reason.includes("In review"))).toBe(true);
});

it("waits for the initial task source after actual policy and agent sources load", async () => {
  follower.dispose();
  const readTasks = tracker.readTasksRecord;
  let release;
  vi.spyOn(tracker, "readTasksRecord").mockImplementationOnce(async (...args) => {
    const record = await readTasks(...args);
    await new Promise((resolve) => { release = resolve; });
    return record;
  });
  const { followWatchedTasks } = await import("../src/core/watchedTaskFollower.js");
  onChange = vi.fn();
  follower = followWatchedTasks({ onChange });
  follower.follow(projects.slice(0, 1));
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  await idle();
  try { expect(onChange).not.toHaveBeenCalled(); }
  finally { release(); }
  await vi.waitFor(() => expect(follower.entries()).toHaveLength(1));
  expect(follower.runningProjectKeys().has(projects[0].projectKey)).toBe(true);
});

it("waits for a pending changed policy before publishing a project joining a warm device", async () => {
  follower.follow(projects.slice(0, 1));
  await idle();
  const record = await tracker.readTasksRecord(deviceId, projects[1].id);
  await tracker.writeTasksRecord(deviceId, projects[1].id, { ...record, tasks: record.tasks.map((task) => ({ ...task, status: "in_review" })) });
  const rule = await import("../src/core/needsYouRule.js");
  const readRule = rule.readNeedsYouRule;
  let release;
  vi.spyOn(rule, "readNeedsYouRule").mockImplementationOnce(async (...args) => {
    const value = await readRule(...args);
    await new Promise((resolve) => { release = resolve; });
    return value;
  });
  await cache.writeCached(address("", "needs-you-rule"), { askedOnly: false });
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  const snapshots = [];
  onChange.mockImplementation(() => {
    const entry = follower.entries().find((item) => item.projectId === projects[1].id);
    if (entry) snapshots.push({ quiet: entry.quiet, reason: entry.reason });
  });
  follower.follow(projects.slice(0, 2));
  await idle();
  try { expect(snapshots).toEqual([]); }
  finally { release(); }
  await vi.waitFor(() => expect(snapshots.length).toBeGreaterThan(0));
  expect(snapshots.every((entry) => !entry.quiet && entry.reason.includes("In review"))).toBe(true);
});

it("waits for a covering agent pass when a project joins after a held roster snapshot", async () => {
  follower.follow(projects.slice(0, 1));
  await idle();
  const readMany = cache.readCachedMany;
  const releases = [];
  vi.spyOn(cache, "readCachedMany").mockImplementation(async (addresses) => {
    const records = await readMany(addresses);
    if (addresses.every(({ kind }) => kind === "row" || kind === "project-agents")) {
      await new Promise((resolve) => releases.push(resolve));
    }
    return records;
  });
  cache.announceCacheEvidence(address(row.entity_id, "row"));
  await vi.waitFor(() => expect(releases).toHaveLength(1));
  const rosterAddress = address(projects[1].entity_id, "project-agents");
  const roster = (await cache.readCached(rosterAddress)).value;
  await cache.writeCached(rosterAddress, { ...roster, agents: roster.agents.map((agent) => ({ ...agent, working: false })) });
  const snapshots = [];
  onChange.mockImplementation(() => {
    const entry = follower.entries().find((item) => item.projectId === projects[1].id);
    if (entry) snapshots.push({ working: entry.working });
  });
  follower.follow(projects.slice(0, 2));
  await idle();
  expect(snapshots).toEqual([]);
  releases[0]();
  try {
    await vi.waitFor(() => expect(releases).toHaveLength(2));
    expect(snapshots).toEqual([]);
    releases[1]();
    await vi.waitFor(() => expect(snapshots.length).toBeGreaterThan(0));
    expect(snapshots.every((entry) => entry.working === false)).toBe(true);
  } finally {
    follower.dispose();
    releases.forEach((release) => release());
  }
});
