// @vitest-environment jsdom
// The inbox reads the real cache and task feed here. A state push is a row
// write, and board.runs is the cached roster even when board.items omits it.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
// Generated from a real bridge MCP tools/call and checked by the Rust test.
const mcpCases = JSON.parse(readFileSync(resolve("../fixtures/watching/mcp_inbox.json"), "utf8"));
let App, writeCached, startFeed, stopFeed, dropFeedDevice, subscribeFeed, mountInboxList, unmountInboxList, setInboxView;
let openCreateWork, adoptDeviceSession, resetDeviceContexts;
let liveFeedSnapshot, stampRow;
let removeFeedRow, patchFeedRow, feedRowTarget, hideProject;
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
  ({ liveFeedSnapshot, stampRow } = await import("../src/core/feedMerge.js"));
  ({ removeFeedRow, patchFeedRow, feedRowTarget } = await import("../src/core/cachedRows.js"));
  ({ hideProject } = await import("../src/core/projectHide.js"));
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
  for (const scenario of mcpCases) {
    it(`renders the ${scenario.label} agent from the bridge MCP through cache and state push`, async () => {
      const view = liveFeedSnapshot(scenario.board, { projects: scenario.projects },
        { workspaces: scenario.workspaces }, deviceId);
      expect(scenario.board.runs[0].agents[0].watched).toBe(scenario.mcp_watched);
      await writeCached(address("feed"), view);
      await writeCached(address("projects"), view.projects);
      await writeCached(address("workspaces"), view.workspaces);
      let snapshot;
      const unsubscribe = subscribeFeed((next) => { snapshot = next; });
      mountInboxList();
      await startFeed();
      await vi.waitFor(() => expect(snapshot?.runs?.[0]?.agents?.[0]?.watched).toBe(scenario.mcp_watched));
      const count = scenario.mcp_watched ? 1 : 0;
      expect(rows()).toHaveLength(count);

      // The bridge's run.get record is the same roster the state-push cache
      // writer stores; keep its watch result instead of replacing it in JS.
      await writeCached(address("row", scenario.row.run_id), stampRow(scenario.row, deviceId));
      await vi.waitFor(() => expect(snapshot?.items?.some((item) => item.run_id === scenario.row.run_id)).toBe(true));
      expect(rows()).toHaveLength(count);
      setInboxView("projects");
      expect(rows()).toHaveLength(count);
      if (count) expect(rows()[0].textContent).toContain(scenario.workspaces[0].name);
      unsubscribe();
    });
  }

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

  it("uses the newer roster across board reads and state pushes on both faces", async () => {
    const { project, workspace, run } = await seed({ watched: true, createdByAgent: true });
    mountInboxList();
    await startFeed();
    const expectBothFaces = async (count) => {
      await vi.waitFor(() => expect(rows()).toHaveLength(count));
      setInboxView("projects");
      await vi.waitFor(() => expect(rows()).toHaveLength(count));
      setInboxView("inbox");
    };
    await expectBothFaces(1);

    // A standalone row is older than this board read. The board omits the run
    // from items, but its roster explicitly says the agent is unwatched.
    await writeCached(address("row", "run-1"), { ...run, kind: "branch" });
    await writeCached(address("feed"), { items: [], runs: [{ ...run, agents: [{ id: "agent-1", watched: false }] }],
      projects: [project], workspaces: [workspace] }, { observedFeedRows: true });
    await expectBothFaces(0);

    // A later state push must win in either direction, even while the board's
    // last roster remains unchanged.
    await writeCached(address("row", "run-1"), { ...run, kind: "branch", agents: [{ id: "agent-1", watched: true }] });
    await expectBothFaces(1);
    await writeCached(address("row", "run-1"), { ...run, kind: "branch", agents: [{ id: "agent-1", watched: false }] });
    await expectBothFaces(0);

    await writeCached(address("feed"), { items: [], runs: [run], projects: [project], workspaces: [workspace] },
      { observedFeedRows: true });
    await expectBothFaces(1);
  });

  const localWriters = [
    { name: "patching A", act: ({ other, run }) => patchFeedRow(deviceId, feedRowTarget(run), { working: true })
      .then(() => removeFeedRow(deviceId, feedRowTarget(other))) },
    { name: "removing B", act: ({ other }) => removeFeedRow(deviceId, feedRowTarget(other)) },
    { name: "hiding B's project", act: () => hideProject({ deviceId, projectKey: `${deviceId}/project-2` }) },
  ];
  for (const boardWatched of [true, false]) for (const writer of localWriters) {
    it(`keeps a newer ${boardWatched ? "muted" : "watched"} push after ${writer.name}`, async () => {
      const { project, workspace, run } = await seed({ watched: boardWatched, createdByAgent: true });
      const projectB = { id: "project-2", project_id: "project-2", name: "Other project",
        deviceId, projectKey: `${deviceId}/project-2` };
      const other = { kind: "branch", entity_id: "run-other", project_id: "project-2",
        branch: "build/other", deviceId, projectKey: projectB.projectKey };
      await writeCached(address("feed"), { items: [{ ...run, kind: "branch", entity_id: "run-1" }, other],
        runs: [run], projects: [project, projectB], workspaces: [workspace] });
      await writeCached(address("projects"), [project, projectB]);
      let snapshot;
      const unsubscribe = subscribeFeed((next) => { snapshot = next; });
      mountInboxList();
      await startFeed();
      await vi.waitFor(() => expect(snapshot?.items?.some((item) => item.entity_id === "run-other")).toBe(true));
      const countA = boardWatched ? 0 : 1;
      const assertBothFaces = async (stage) => {
        await vi.waitFor(() => expect(rows().filter((row) => row.textContent.includes("Agent work")),
          `workspace face ${stage}`).toHaveLength(countA));
        setInboxView("projects");
        await vi.waitFor(() => expect(rows().filter((row) => row.textContent.includes("Agent work")),
          `projects face ${stage}`).toHaveLength(countA));
        setInboxView("inbox");
      };

      // A's pushed watch state is newer than its board roster. Removing B
      // rewrites the feed but does not observe A again.
      await writeCached(address("row", "run-1"), { ...run, kind: "branch",
        agents: [{ id: "agent-1", watched: !boardWatched }] });
      await vi.waitFor(() => expect(snapshot?.items?.find((item) => item.run_id === "run-1")?.agents?.[0]?.watched)
        .toBe(!boardWatched));
      await assertBothFaces("before mutation");
      await writer.act({ other, run });
      await vi.waitFor(() => expect(snapshot?.items?.some((item) => item.entity_id === "run-other")).toBe(false));
      await assertBothFaces("after mutation");
      unsubscribe();
    });
  }

  for (const boardWatched of [true, false]) {
    it(`keeps A ${boardWatched ? "muted" : "watched"} when B's removal is refused after a new board`, async () => {
      const { project, workspace, run } = await seed({ watched: boardWatched, createdByAgent: true });
      const itemA = { ...run, kind: "branch", entity_id: "run-1" };
      const itemB = { kind: "branch", entity_id: "run-other", project_id: "project-1",
        branch: "build/other", deviceId, projectKey: project.projectKey };
      await writeCached(address("feed"), { items: [itemA, itemB], runs: [run],
        projects: [project], workspaces: [workspace] }, { observedFeedRows: true });
      let snapshot;
      const unsubscribe = subscribeFeed((next) => { snapshot = next; });
      mountInboxList();
      await startFeed();
      await vi.waitFor(() => expect(snapshot?.items?.some((item) => item.entity_id === "run-other")).toBe(true));

      const undo = await removeFeedRow(deviceId, feedRowTarget(itemB));
      await vi.waitFor(() => expect(snapshot?.items?.some((item) => item.entity_id === "run-other")).toBe(false));
      const newlyWatched = !boardWatched;
      await writeCached(address("feed"), { items: [],
        runs: [{ ...run, agents: [{ id: "agent-1", watched: newlyWatched }] }],
        projects: [project], workspaces: [workspace] }, { observedFeedRows: true });
      await vi.waitFor(() => {
        expect(snapshot?.items?.some((item) => item.entity_id === "run-1")).toBe(false);
        expect(snapshot?.runs?.[0]?.agents?.[0]?.watched).toBe(newlyWatched);
      });
      const countA = newlyWatched ? 1 : 0;
      const assertBothFaces = async (stage) => {
        await vi.waitFor(() => expect(rows().filter((row) => row.textContent.includes("Agent work")),
          `workspace face ${stage}`).toHaveLength(countA));
        setInboxView("projects");
        await vi.waitFor(() => expect(rows().filter((row) => row.textContent.includes("Agent work")),
          `projects face ${stage}`).toHaveLength(countA));
        setInboxView("inbox");
      };
      await assertBothFaces("before refusal");

      await undo();
      await vi.waitFor(() => expect(snapshot?.items?.some((item) => item.entity_id === "run-other")).toBe(true));
      expect(snapshot.items.some((item) => item.entity_id === "run-1")).toBe(false);
      expect(snapshot.runs[0].agents[0].watched).toBe(newlyWatched);
      await assertBothFaces("after refusal");
      unsubscribe();
    });
  }

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
