// @vitest-environment jsdom
// The inbox reads the real cache and task feed here. A state push is a row
// write, and board.runs is the cached roster even when board.items omits it.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
let App, writeCached, startFeed, stopFeed, dropFeedDevice, subscribeFeed, mountInboxList, unmountInboxList, setInboxView;
let openCreateWork, adoptDeviceSession, resetDeviceContexts;
const deviceId = "watching-device";
const address = (kind, entityId = "") => ({ deviceId, entityId, kind });
const rows = () => [...document.querySelectorAll("#inbox-list .inbox-entry")];

async function seed({ watched, agents = [{ id: "agent-1", watched }], createdByAgent = false, entityId = "run-1" } = {}) {
  const project = { id: "project-1", project_id: "project-1", name: "Payments", deviceId, projectKey: `${deviceId}/project-1` };
  const workspace = { id: "workspace-1", project_id: "project-1", name: "Agent work", status: "ready",
    entity_id: entityId, created_by_agent: createdByAgent, deviceId, projectKey: project.projectKey,
    workspaceKey: `${deviceId}/workspace-1` };
  const run = { run_id: "run-1", project_id: "project-1", agents, deviceId, projectKey: project.projectKey };
  await writeCached(address("feed"), { items: [], runs: entityId ? [run] : [], projects: [project], workspaces: [workspace] });
  await writeCached(address("projects"), [project]);
  await writeCached(address("workspaces"), [workspace]);
  return { project, workspace, run };
}

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = bodyHtml;
  ({ App } = await import("../src/app.js"));
  ({ writeCached } = await import("../src/core/localCache.js"));
  ({ startFeed, stopFeed, dropFeedDevice, subscribeFeed } = await import("../src/core/taskFeed.js"));
  ({ mountInboxList, unmountInboxList, setInboxView } = await import("../src/core/inboxView.js"));
  ({ openCreateWork } = await import("../src/core/createWork.js"));
  ({ adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js"));
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

describe("cached watching on both inbox faces", () => {
  it("hides an agent-created workspace without agents on cold replay and a workspace push", async () => {
    const { workspace } = await seed({ agents: [], createdByAgent: true, entityId: null });
    let snapshot;
    const unsubscribe = subscribeFeed((next) => { snapshot = next; });
    mountInboxList();
    await startFeed();
    await vi.waitFor(() => expect(snapshot?.workspaces).toHaveLength(1));
    expect(rows()).toHaveLength(0);

    await writeCached(address("workspaces"), [{ ...workspace, status: "active" }]);
    await vi.waitFor(() => expect(snapshot?.workspaces?.[0]?.status).toBe("active"));
    expect(rows()).toHaveLength(0);
    setInboxView("projects");
    expect(rows()).toHaveLength(0);
    unsubscribe();
  });

  it("hides an unwatched agent's workspace from a cold cache and after a pushed row", async () => {
    const { run } = await seed({ watched: false, createdByAgent: true });
    let snapshot;
    const unsubscribe = subscribeFeed((next) => { snapshot = next; });
    mountInboxList();
    await startFeed();
    await vi.waitFor(() => expect(snapshot?.runs).toHaveLength(1));
    expect(rows()).toHaveLength(0);

    // The state push writes a standalone row even though board.items omitted
    // the muted run. The inbox must inspect the roster, not that row's presence.
    await writeCached(address("row", "run-1"), { ...run, kind: "branch", working: true });
    await vi.waitFor(() => expect(snapshot?.items).toHaveLength(1));
    expect(rows()).toHaveLength(0);
    setInboxView("projects");
    expect(rows()).toHaveLength(0);
    unsubscribe();
  });

  it("shows a watched agent's workspace after the same cold replay and push", async () => {
    const { run } = await seed({ watched: true, createdByAgent: true });
    mountInboxList();
    await startFeed();
    await vi.waitFor(() => expect(rows().map((row) => row.textContent)).toEqual([expect.stringContaining("Agent work")]));
    await writeCached(address("row", "run-1"), { ...run, kind: "branch", working: true });
    await vi.waitFor(() => expect(rows()[0]?.textContent).toContain("Agent work"));
    setInboxView("projects");
    expect(rows()).toHaveLength(1);
  });

  it("updates an inbox row when a cached agent is watched or unwatched", async () => {
    const { run } = await seed({ watched: false, createdByAgent: true });
    mountInboxList();
    await startFeed();
    await writeCached(address("row", "run-1"), { ...run, kind: "branch", agents: [{ id: "agent-1", watched: true }] });
    await vi.waitFor(() => expect(rows()).toHaveLength(1));
    await writeCached(address("row", "run-1"), { ...run, kind: "branch", agents: [{ id: "agent-1", watched: false }] });
    await vi.waitFor(() => expect(rows()).toHaveLength(0));
  });

  it("shows a UI-created, agentless workspace before the next board read", async () => {
    await seed({ agents: [] });
    mountInboxList();
    await startFeed();
    const bridge = vi.fn(async (method) => method === "workspace.create"
      ? { id: "workspace-new", project_id: "project-1", name: "My workspace", status: "ready", created_by_agent: false, directories: [] }
      : {});
    adoptDeviceSession({ deviceId, call: bridge, close: () => {}, peer: () => {}, onCarrier: () => {} });

    openCreateWork({ projectId: "project-1", deviceId, projectName: "Payments", navigate: vi.fn() });
    document.querySelector("#create-scrim [data-create-go]").click();
    await vi.waitFor(() => expect(rows().some((row) => row.textContent.includes("My workspace"))).toBe(true));
    expect(bridge).toHaveBeenCalledWith("workspace.create", { project_id: "project-1", name: "" });
    expect(bridge.mock.calls.some(([method]) => method === "board.list")).toBe(false);
  });
});
