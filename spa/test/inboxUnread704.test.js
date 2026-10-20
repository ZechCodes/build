// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

vi.mock("../src/core/cacheSync.js", () => ({ syncDevice: vi.fn(), routeChanged() {} }));
let cache, feed, last, unsubscribe;
let cleanups;
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
  cleanups = [];
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  feed = await import("../src/core/taskFeed.js");
  await cache.writeCached(cache.DEVICES_ADDRESS, [{ id: deviceId }]);
  await cache.writeCached(address("projects"), [project]);
  await cache.writeCached(address("agent-lineage-support"), { namesMakers: true });
  unsubscribe = feed.subscribeFeed((value) => { last = value; });
});
afterEach(() => { cleanups.forEach((stop) => stop()); unsubscribe(); feed.stopFeed(); vi.restoreAllMocks(); });

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

const selectedConversation = (over = {}) => ({ deviceId, entityId: "owner", agentId: "pin",
  conversationId: "pin-history", threadId: "pin-thread", ...over });

async function recordInboxPaints() {
  const { projectAgentEntries } = await import("../src/core/inboxProjectAgent.js");
  const { inboxRowHtml } = await import("../src/core/inbox.js");
  const { projectsUnreadCount } = await import("../src/core/inboxProjects.js");
  const { publishInboxAttentionCount, subscribeInboxAttentionCount } = await import("../src/core/inboxAttention.js");
  const paints = [], totals = [];
  cleanups.push(subscribeInboxAttentionCount((count) => totals.push(count)));
  cleanups.push(feed.subscribeFeed((value) => {
    const rows = projectAgentEntries(value.projects, value.items, value.runs);
    const html = new DOMParser().parseFromString(rows.map(inboxRowHtml).join(""), "text/html");
    paints.push(!!html.querySelector(".inbox-status-unread"));
    publishInboxAttentionCount(projectsUnreadCount(rows, value.projects));
  }));
  return { paints, totals };
}

it.each(["state first", "transcript first"])("never paints an open, present conversation arrival unread (%s)", async (order) => {
  const { registerSyncThread } = await import("../src/core/syncVisibleScopes.js");
  await writeRow(owner([agent("pin")]));
  // Inbox subscribes before the rail. Eligibility already exists when state lands;
  // no entity.seen reply (or even a connection) can rescue this fixture.
  const observed = await recordInboxPaints();
  cleanups.push(registerSyncThread(() => selectedConversation(), () => true));
  await feed.startFeed();
  const transcript = () => cache.writeCached({ ...address("thread", "owner"), sub: "pin-history" },
    { thread_id: "pin-thread", items: [{ kind: "message", sequence: 2, role: "agent", body: "Arrived" }] });
  if (order === "transcript first") await transcript();
  await writeRow(owner([agent("pin", { unread_count: 1 })]));
  if (order === "state first") await transcript();
  await vi.waitFor(() => expect(last.items[0].agents[0].unread_count).toBe(1));
  // Check every delivered repaint, then the next frame, while the count is
  // still 1 on disk. An eventual cleared assertion would miss the flash.
  feed.deliverFeed();
  await new Promise((resolve) => requestAnimationFrame(resolve));
  expect(observed.paints.length).toBeGreaterThan(1);
  expect(observed.paints.every((unread) => !unread)).toBe(true);
  expect(observed.totals.every((count) => count === 0)).toBe(true);
  expect((await cache.readCached(address("row", "owner"))).value.agents[0].unread_count).toBe(1);
});

it.each(["hidden thread", "absent reader", "other device", "other history", "other generation"])(
  "keeps ordinary unread for %s", async (condition) => {
    const { registerSyncThread } = await import("../src/core/syncVisibleScopes.js");
    vi.spyOn(document, "hasFocus").mockReturnValue(condition !== "absent reader");
    const identity = selectedConversation({
      ...(condition === "other device" ? { deviceId: "other" } : {}),
      ...(condition === "other history" ? { conversationId: "other-history" } : {}),
      ...(condition === "other generation" ? { threadId: "other-thread" } : {}),
    });
    cleanups.push(registerSyncThread(() => identity, () => condition !== "hidden thread"));
    await writeRow(owner([agent("pin", { unread_count: 1 })]));
    await feed.startFeed();
    expect(await projectDot()).toBe(true);
    expect((await entries())[0].ownUnreadCount).toBe(1);
  },
);

it("repaints cached unread immediately when the reader leaves and returns, with no state write", async () => {
  const { registerSyncThread } = await import("../src/core/syncVisibleScopes.js");
  const focus = vi.spyOn(document, "hasFocus").mockReturnValue(true);
  cleanups.push(registerSyncThread(() => selectedConversation(), () => true));
  await writeRow(owner([agent("pin", { unread_count: 1 })]));
  const { paints, totals } = await recordInboxPaints();
  await feed.startFeed();
  expect(paints.at(-1)).toBe(false);
  focus.mockReturnValue(false);
  window.dispatchEvent(new Event("blur"));
  expect(paints.at(-1)).toBe(true);
  expect(totals.at(-1)).toBe(1);
  focus.mockReturnValue(true);
  window.dispatchEvent(new Event("focus"));
  expect(paints.at(-1)).toBe(false);
  expect(totals.at(-1)).toBe(0);
});

it("suppresses only the selected canonical worker, preserving siblings and ancestor unread", async () => {
  const { registerSyncThread, notifyThreadReaders } = await import("../src/core/syncVisibleScopes.js");
  let selected = selectedConversation();
  const stop = registerSyncThread(() => selected, () => true);
  cleanups.push(stop);
  await writeRow(owner([agent("pin")]));
  await cache.writeCached(address("project-agents-support"), { projectWorkers: true });
  const roster = (siblingUnread) => ({ entity_id: "owner", pinned_agent_id: "pin",
    agents: [agent("pin"), agent("old-worker", { conversation_id: "shared", unread_count: 9, created_by: "pin" }),
      agent("worker", { conversation_id: "shared", unread_count: 1, created_by: "pin" }),
      agent("sibling", { unread_count: siblingUnread, created_by: "pin" })] });
  await cache.writeCached(address("project-agents", "owner"), roster(1));
  await feed.startFeed();
  // Reading the coordinator cannot hide news on either worker.
  expect((await entries())[0].ownUnreadCount).toBe(2);
  expect(await projectDot()).toBe(true);
  selected = selectedConversation({ agentId: "old-worker", conversationId: "shared", threadId: "worker-thread" });
  notifyThreadReaders();
  expect((await entries())[0].ownUnreadCount).toBe(1);
  expect(await projectDot()).toBe(true);
  await cache.writeCached(address("project-agents", "owner"), roster(0));
  await vi.waitFor(async () => expect(await projectDot()).toBe(false));
  expect((await entries())[0].ownUnreadCount).toBe(0);
  stop();
  expect((await entries())[0].ownUnreadCount).toBe(1);
  expect(await projectDot()).toBe(true);
});

it("restores unread when the document hides, without changing the cached count", async () => {
  const { registerSyncThread } = await import("../src/core/syncVisibleScopes.js");
  const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(false);
  cleanups.push(registerSyncThread(() => selectedConversation(), () => true));
  await writeRow(owner([agent("pin", { unread_count: 1 })]));
  const { paints, totals } = await recordInboxPaints();
  await feed.startFeed();
  hidden.mockReturnValue(true);
  document.dispatchEvent(new Event("visibilitychange"));
  expect(paints.at(-1)).toBe(true);
  expect(totals.at(-1)).toBe(1);
  hidden.mockReturnValue(false);
  document.dispatchEvent(new Event("visibilitychange"));
  expect(paints.at(-1)).toBe(false);
  expect(totals.at(-1)).toBe(0);
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
