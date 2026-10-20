// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

vi.mock("../src/core/cacheSync.js", () => ({ syncDevice: vi.fn(), routeChanged() {} }));
let cache, feed, last, unsubscribe;
const deviceId = "unread-704";
const projectKey = `${deviceId}/project`;
const project = { id: "project", name: "Build", entity_id: "owner", deviceId, projectKey };
const agent = (id, over = {}) => ({ id, conversation_id: `${id}-history`, thread_id: `${id}-thread`,
  watched: true, unread_count: 0, read_through_sequence: 1, working: false, ...over });
const row = (entityId, agents, over = {}) => ({ kind: "branch", entity_id: entityId,
  project_id: project.id, projectKey, deviceId, agents, ...over });
const address = (kind, entityId = "") => ({ deviceId, entityId, kind });
const owner = (agents) => row("owner", agents);
const writeRow = (value) => cache.writeCached(address("row", value.entity_id), value);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  feed = await import("../src/core/taskFeed.js");
  await cache.writeCached(cache.DEVICES_ADDRESS, [{ id: deviceId }]);
  await cache.writeCached(address("projects"), [project]);
  await cache.writeCached(address("agent-lineage-support"), { namesMakers: true });
  unsubscribe = feed.subscribeFeed((value) => { last = value; });
});
afterEach(() => { unsubscribe(); feed.stopFeed(); });

async function entries() {
  const { projectAgentEntries } = await import("../src/core/inboxProjectAgent.js");
  const { watchedWorkspaceEntries } = await import("../src/core/inbox.js");
  return [...projectAgentEntries(last.projects, last.items, last.runs),
    ...watchedWorkspaceEntries(last.workspaces, last.projects, last.items, last.runs)];
}

async function projectDot() {
  const { inboxRowHtml } = await import("../src/core/inbox.js");
  const html = new DOMParser().parseFromString(inboxRowHtml((await entries())[0]), "text/html");
  return !!html.querySelector(".inbox-status-unread");
}

it("shows a new message while away, then clears on another client's cached read and stays clear after reload", async () => {
  await writeRow(owner([agent("pin")]));
  await feed.startFeed();
  expect(await projectDot()).toBe(false);
  await writeRow(owner([agent("pin", { unread_count: 1 })]));
  await vi.waitFor(async () => expect(await projectDot()).toBe(true));
  await writeRow(owner([agent("pin", { read_through_sequence: 2 })]));
  await vi.waitFor(async () => expect(await projectDot()).toBe(false));
  feed.stopFeed();
  await feed.startFeed();
  expect(await projectDot()).toBe(false);
  expect((await entries())[0].ownUnreadCount).toBe(0);
});

it("reports visible new messages even before the roster's unread count arrives", async () => {
  const { createChatReadReporter, hasReadProgress } = await import("../src/core/chatReadReporter.js");
  await writeRow(owner([agent("pin")]));
  await feed.startFeed();
  const reporter = createChatReadReporter();
  const digest = () => last.items.find((item) => item.entity_id === "owner").agents[0];
  expect(hasReadProgress(digest(), 2)).toBe(true);
  await reporter.report(2, 1, async () => {
    await writeRow(owner([agent("pin", { read_through_sequence: 2 })]));
    await vi.waitFor(() => expect(digest().read_through_sequence).toBe(2));
    return true;
  }, () => digest().read_through_sequence);
  expect(await projectDot()).toBe(false);
});

it("includes watched project workers from the complete roster without reviving stale board counts", async () => {
  await writeRow(owner([agent("pin", { unread_count: 9 })]));
  await cache.writeCached(address("project-agents-support"), { projectWorkers: true });
  const roster = (unread) => ({ entity_id: "owner", pinned_agent_id: "pin",
    agents: [agent("pin"), agent("worker", { created_by: "pin", unread_count: unread }),
      agent("unwatched", { watched: false, unread_count: 10 })] });
  await cache.writeCached(address("project-agents", "owner"), roster(1));
  await feed.startFeed();
  expect((await entries())[0].ownUnreadCount).toBe(1);
  await cache.writeCached(address("project-agents", "owner"), roster(0));
  await vi.waitFor(async () => expect(await projectDot()).toBe(false));
  await feed.startFeed();
  expect(await projectDot()).toBe(false);
});

it("rolls watched descendant unread into the parent dot without counting the same message twice", async () => {
  const workspace = { id: "workspace", workspaceKey: `${deviceId}/workspace`, deviceId, projectKey,
    project_id: project.id, entity_id: "child-owner", status: "ready", name: "Worker" };
  await cache.writeCached(address("workspaces"), [workspace]);
  await writeRow(owner([agent("pin")]));
  await writeRow(row("child-owner", [agent("worker", { created_by: "pin", unread_count: 2 })]));
  await feed.startFeed();
  expect(await projectDot()).toBe(true);
  const { projectsUnreadCount } = await import("../src/core/inboxProjects.js");
  expect(projectsUnreadCount(await entries(), last.projects)).toBe(2);
  expect((await entries())[0].ownUnreadCount).toBe(0);
  expect((await cache.readCached(address("row", "owner"))).value.agents[0]).not.toHaveProperty("agents_unread");
  await writeRow(row("child-owner", [agent("worker", { created_by: "pin", read_through_sequence: 3 })]));
  await vi.waitFor(async () => expect(await projectDot()).toBe(false));
});

it("uses complete project workers in transitive lineage and decorates their parent roster", async () => {
  await writeRow(owner([agent("pin")]));
  await cache.writeCached(address("project-agents-support"), { projectWorkers: true });
  await cache.writeCached(address("project-agents", "owner"), { entity_id: "owner", pinned_agent_id: "pin",
    agents: [agent("pin"), agent("middle", { created_by: "pin" })] });
  await writeRow(row("child-owner", [agent("worker", { created_by: "middle", unread_count: 1, working: true })]));
  await feed.startFeed();
  expect(await projectDot()).toBe(true);
  expect((await entries())[0].working).toBe(true);
  await cache.writeCached(address("agent-lineage-support"), { namesMakers: false });
  await vi.waitFor(async () => expect(await projectDot()).toBe(false));
});

it.each([
  { watched: false },
  { project_id: "other", projectKey: `${deviceId}/other` },
])("does not roll up unwatched or other-project messages: %j", async (over) => {
  await writeRow(owner([agent("pin")]));
  await writeRow(row("child-owner", [agent("worker", { created_by: "pin", unread_count: 2, ...over })], over));
  await feed.startFeed();
  expect(await projectDot()).toBe(false);
});

it("follows canonical creator aliases and never resurrects an older alias's unread", async () => {
  await writeRow(owner([agent("pin")]));
  await cache.writeCached(address("project-agents-support"), { projectWorkers: true });
  await cache.writeCached(address("project-agents", "owner"), { entity_id: "owner", pinned_agent_id: "pin",
    agents: [agent("pin"), agent("older", { created_by: "pin", conversation_id: "shared", unread_count: 7 }),
      agent("canonical", { created_by: "pin", conversation_id: "shared" })] });
  await writeRow(row("child-owner", [agent("worker", { created_by: "older", unread_count: 1 })]));
  await feed.startFeed();
  expect(await projectDot()).toBe(true);
  expect((await entries())[0].ownUnreadCount).toBe(0);
  await writeRow(row("child-owner", [agent("worker", { created_by: "older", read_through_sequence: 2 })]));
  await vi.waitFor(async () => expect(await projectDot()).toBe(false));
});
