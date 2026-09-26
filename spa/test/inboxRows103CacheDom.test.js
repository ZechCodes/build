// @vitest-environment jsdom
// #103 wired end to end, with nothing mocked: the real cache, the real task
// feed and the real rail. The workspace row's running count and watched unread,
// the project agent's row, and the projects face's head badge all paint from
// what the cache holds, and a state push written to the cache repaints them.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
let App, writeCached, startFeed, stopFeed, dropFeedDevice, mountInboxList, unmountInboxList, setInboxView;
let resetDeviceContexts, stampRow;
const deviceId = "rows-103-device";
const projectKey = `${deviceId}/project-1`;
const address = (kind, entityId = "") => ({ deviceId, entityId, kind });
const rows = () => [...document.querySelectorAll("#inbox-list .inbox-entry")];
const rowByKey = (key) => rows().find((row) => row.dataset.key === key) || null;
const badgeIn = (element) => element?.querySelector(".inbox-actions > .inbox-unread")?.textContent || null;
const head = () => document.querySelector(`#inbox-list .inbox-project[data-project="${projectKey}"] .inbox-project-head`);

const agent = (id, over = {}) => ({ id, watched: true, working: false, unread_count: 0, ...over });

async function seed() {
  const now = Date.now();
  const project = { id: "project-1", project_id: "project-1", name: "Payments", entity_id: "run-p",
    session_started_ms: now - 60_000, last_activity_ms: now - 60_000, deviceId, projectKey };
  const workspace = { id: "workspace-1", project_id: "project-1", name: "Checkout flow", status: "ready",
    entity_id: "run-1", can_finish: true, work_summary: { pushes: 1, behind: 0, additions: 12, deletions: 3 },
    session_started_ms: now - 120_000, last_activity_ms: now - 60_000,
    deviceId, projectKey, workspaceKey: `${deviceId}/workspace-1` };
  const workspaceRun = { kind: "branch", run_id: "run-1", project_id: "project-1", deviceId, projectKey, agents: [
    agent("a", { working: true, unread_count: 2 }),
    agent("b", { working: true, unread_count: 5, watched: false }),
    agent("c", { unread_count: 1 }),
  ] };
  const projectRun = { kind: "branch", run_id: "run-p", project_id: "project-1", deviceId, projectKey,
    session_started_ms: now - 90_000, last_activity_ms: now - 30_000,
    agents: [agent("project-agent", { unread_count: 4 })] };
  await writeCached(address("feed"), { items: [workspaceRun, projectRun], runs: [], projects: [project], workspaces: [workspace] });
  await writeCached(address("projects"), [project]);
  await writeCached(address("workspaces"), [workspace]);
  return { projectRun };
}

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = bodyHtml;
  ({ App } = await import("../src/app.js"));
  ({ writeCached } = await import("../src/core/localCache.js"));
  ({ startFeed, stopFeed, dropFeedDevice } = await import("../src/core/taskFeed.js"));
  ({ mountInboxList, unmountInboxList, setInboxView } = await import("../src/core/inboxView.js"));
  ({ resetDeviceContexts } = await import("../src/core/deviceContexts.js"));
  ({ stampRow } = await import("../src/core/feedMerge.js"));
  resetDeviceContexts();
  App.route = { name: "inbox" };
  App.devices = [{ id: deviceId, name: "Laptop", status: "online" }];
  App.selectedDeviceId = deviceId;
  App.deviceFilter = null;
  setInboxView("inbox");
});

afterEach(() => {
  unmountInboxList();
  stopFeed();
  dropFeedDevice(deviceId);
  resetDeviceContexts();
});

describe("#103 inbox rows, from the cache", () => {
  it("paints the workspace row's running count and watched unread, and the project agent's row", async () => {
    await seed();
    mountInboxList();
    await startFeed();
    await vi.waitFor(() => expect(rows().map((row) => row.dataset.key)).toEqual([
      `workspace:${deviceId}/workspace-1`,
      `project-agent:${projectKey}`,
    ]));
    const workspaceRow = rowByKey(`workspace:${deviceId}/workspace-1`);
    expect(workspaceRow.querySelector(".inbox-facts").textContent).toBe("↑1 ↓0 +12 −3 · 2 running");
    expect(badgeIn(workspaceRow)).toBe("3");
    expect(workspaceRow.querySelector(".inbox-actions > .inbox-workspace-done + .inbox-unread")).not.toBeNull();

    const agentRow = rowByKey(`project-agent:${projectKey}`);
    expect(agentRow.querySelector(".stitle").textContent).toBe("Payments");
    expect(agentRow.querySelector(".inbox-facts, [data-workspace-done]")).toBeNull();
    expect(badgeIn(agentRow)).toBe("4");
  });

  it("repaints the project agent's row from a state push written to the cache", async () => {
    const { projectRun } = await seed();
    mountInboxList();
    await startFeed();
    await vi.waitFor(() => expect(badgeIn(rowByKey(`project-agent:${projectKey}`))).toBe("4"));
    await writeCached(address("row", "run-p"), stampRow({ ...projectRun,
      agents: [agent("project-agent", { unread_count: 6 })] }, deviceId));
    await vi.waitFor(() => expect(badgeIn(rowByKey(`project-agent:${projectKey}`))).toBe("6"));
  });

  it("makes the project agent the head of its block, badged by the fold", async () => {
    await seed();
    mountInboxList();
    await startFeed();
    setInboxView("projects");
    await vi.waitFor(() => expect(head()?.querySelector(".inbox-unread")?.textContent).toBe("4"));
    expect(rows().map((row) => row.dataset.key)).toEqual([`workspace:${deviceId}/workspace-1`]);

    head().querySelector("[data-project-fold]").click();
    await vi.waitFor(() => expect(head().querySelector(".inbox-unread")?.textContent).toBe("7"));
    head().querySelector("[data-project-fold]").click();
    await vi.waitFor(() => expect(head().querySelector(".inbox-unread")?.textContent).toBe("4"));
  });
});
