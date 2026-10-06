// @vitest-environment jsdom
// #183, unmocked: the real cache, feed, task follower and rail, with no
// session at all, so every number comes from what the cache holds. The top
// badge the rail publishes counts all watched news — open or folded, on either
// face — independently of the status dots (#380). The report's shape is two
// projects, a Needs-you row with nothing unread, a watched task nobody holds
// with no row, a Done
// task a 1.29 bridge still counts, a workspace in Recent and a folded block.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const DEVICE = "badge-183-device";
const WAIT = { timeout: 5000, interval: 20 };
const HOUR = 60 * 60 * 1000;
const address = (kind, entityId = "") => ({ deviceId: DEVICE, entityId, kind });
const keyOf = (projectId) => `${DEVICE}/${projectId}`;
const agent = (id, over = {}) => ({ id, watched: true, working: false, unread_count: 0, ...over });

let modules;
let published;

const head = (projectId) =>
  document.querySelector(`#inbox-list .inbox-project[data-project="${keyOf(projectId)}"] .inbox-project-head`);
const headDot = (projectId) => head(projectId)?.querySelector(".inbox-status-dot");
const block = (projectId) => head(projectId)?.closest(".inbox-project") || null;
const row = (key) => document.querySelector(`#inbox-list .inbox-entry[data-key="${key}"]`);

async function seed({ buildUnread = 1, buildWorking = false, oldWorking = false } = {}) {
  const now = Date.now();
  const project = (id, name) => ({ id, project_id: id, name, entity_id: `run-${id}`,
    session_started_ms: now - HOUR, last_activity_ms: now - 60_000, deviceId: DEVICE, projectKey: keyOf(id) });
  const workspace = (id, projectId, lastMs) => ({ id, project_id: projectId, name: id, status: "ready",
    entity_id: `run-${id}`, work_summary: { pushes: 0, behind: 0, additions: 0, deletions: 0 },
    session_started_ms: lastMs - HOUR, last_activity_ms: lastMs, deviceId: DEVICE, projectKey: keyOf(projectId),
    workspaceKey: `${DEVICE}/${id}` });
  const run = (runId, projectId, agents) => ({ kind: "branch", run_id: runId, project_id: projectId,
    deviceId: DEVICE, projectKey: keyOf(projectId), agents });
  const projects = [project("build", "Build"), project("smarter", "smarter-dev")];
  // Build's workspace has said nothing for two days: it is in Recent.
  const workspaces = [workspace("old-work", "build", now - 48 * HOUR), workspace("bot-fix", "smarter", now - 60_000)];
  const items = [
    run("run-build", "build", [agent("build-agent", { unread_count: buildUnread, working: buildWorking })]),
    run("run-old-work", "build", [agent("old-agent", { unread_count: 2, working: oldWorking })]),
    run("run-smarter", "smarter", [agent("smarter-agent", { unread_count: 1 })]),
    run("run-bot-fix", "smarter", [agent("bot-agent", { unread_count: 1 })]),
  ];
  await modules.cache.writeCached(address("feed"), { items, runs: [], projects, workspaces });
  await modules.cache.writeCached(address("projects"), projects);
  await modules.cache.writeCached(address("workspaces"), workspaces);
  const tasks = [
    // #159: in review, everything read — a Needs-you row with no unread.
    task({ id: "i-159", number: 159, title: "Review me", watched: true, status: "in_review", unread_count: 0,
      updated_at: new Date(now - 120_000).toISOString() }),
    // #113: watched, nobody's, 2 unread, and no row.
    task({ id: "i-113", number: 113, title: "Milestones", watched: true, status: "ready", unread_count: 2 }),
    // #50: Done; a 1.29 bridge sends its unread anyway.
    task({ id: "i-50", number: 50, title: "Finished", watched: true, status: "done", unread_count: 803 }),
  ];
  await modules.tracker.writeTasksRecord(DEVICE, "build", modules.tracker.tasksRecord(tasks, []));
}

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = bodyHtml;
  const { App } = await import("../src/app.js");
  Object.assign(App, { route: { name: "inbox" }, devices: [{ id: DEVICE, name: "Laptop", status: "online" }],
    selectedDeviceId: DEVICE, deviceFilter: null });
  modules = {
    cache: await import("../src/core/localCache.js"),
    tracker: await import("../src/core/trackerCache.js"),
    taskFeed: await import("../src/core/taskFeed.js"),
    inboxView: await import("../src/core/inboxView.js"),
    attention: await import("../src/core/inboxAttention.js"),
    deviceContexts: await import("../src/core/deviceContexts.js"),
  };
  modules.deviceContexts.resetDeviceContexts();
  modules.attention.subscribeInboxAttentionCount((count) => {
    published = count;
  });
  await seed();
  modules.inboxView.setInboxView("projects");
  modules.inboxView.mountInboxList();
  await modules.taskFeed.startFeed();
});

afterEach(() => {
  modules.inboxView.unmountInboxList();
  modules.taskFeed.stopFeed();
  modules.taskFeed.dropFeedDevice(DEVICE);
  modules.deviceContexts.resetDeviceContexts();
});

const fold = async (projectId, folded) => {
  if (block(projectId).classList.contains("inbox-folded") === folded) return;
  head(projectId).querySelector("[data-project-fold]").click();
  await vi.waitFor(() => expect(block(projectId).classList.contains("inbox-folded")).toBe(folded), WAIT);
};

// Build: its agent 1 + #113 2 + the Recent workspace 2. #159 is fully read.
// smarter-dev: its agent 1 + its workspace 1. #50 counts nowhere.
const BUILD = 1 + 2 + 2;
const SMARTER = 1 + 1;

const expectHeadStatus = (projectId, { unread, running = false }) => {
  const dot = headDot(projectId);
  expect(head(projectId).querySelector(".inbox-unread, .sdot")).toBeNull();
  if (!unread && !running) {
    expect(dot).toBeNull();
    return;
  }
  expect(dot).not.toBeNull();
  expect(dot.classList.contains("inbox-status-unread")).toBe(unread);
  expect(dot.classList.contains("inbox-status-running")).toBe(running);
  expect(head(projectId).lastElementChild).toBe(dot);
};

describe("the inbox's top badge", () => {
  it("counts all watched news while open heads show their own agent's dot", async () => {
    await vi.waitFor(() => expect(row("tracker_task:i-159")).not.toBe(null), WAIT);
    await fold("build", false);
    await fold("smarter", false);
    await vi.waitFor(() => expect(published).toBe(BUILD + SMARTER), WAIT);
    expectHeadStatus("build", { unread: true });
    expectHeadStatus("smarter", { unread: true });
    expect(row("tracker_task:i-113")).toBe(null);
    expect(row(`workspace:${DEVICE}/old-work`)?.closest(".inbox-recent")).not.toBe(null);
    expect(published).toBe(BUILD + SMARTER);
  });

  it("keeps the numeric total identical while folded heads show aggregate dots", async () => {
    await vi.waitFor(() => expect(published).toBe(BUILD + SMARTER), WAIT);
    await fold("smarter", true);
    await fold("build", true);
    expectHeadStatus("build", { unread: true });
    expectHeadStatus("smarter", { unread: true });
    expect(published).toBe(BUILD + SMARTER);
    await fold("build", false);
    expect(published).toBe(BUILD + SMARTER);
  });

  it("separates an expanded head's own unread from watched news and Recent activity", async () => {
    await vi.waitFor(() => expect(published).toBe(BUILD + SMARTER), WAIT);
    await seed({ buildUnread: 0, oldWorking: true });
    await fold("build", false);
    await vi.waitFor(() => expect(published).toBe(BUILD + SMARTER - 1), WAIT);
    expectHeadStatus("build", { unread: false });

    await fold("build", true);
    expectHeadStatus("build", { unread: true, running: true });
    expect(published).toBe(BUILD + SMARTER - 1);

    await seed({ buildUnread: 0, oldWorking: false });
    await vi.waitFor(() => expectHeadStatus("build", { unread: true, running: false }), WAIT);
    expect(published).toBe(BUILD + SMARTER - 1);
  });

  it("stays the same number on the flat face", async () => {
    await vi.waitFor(() => expect(published).toBe(BUILD + SMARTER), WAIT);
    modules.inboxView.setInboxView("inbox");
    await vi.waitFor(() => expect(row(`project-agent:${keyOf("build")}`)).not.toBe(null), WAIT);
    expect(published).toBe(BUILD + SMARTER);
  });
});
