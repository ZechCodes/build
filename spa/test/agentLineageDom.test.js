/** @vitest-environment jsdom */
// The lineage reader (#216): who made whom, read off the device's cached rows
// and nothing else. A pull or a push rewrites a row record; the reader hears
// the record and supplies again, with no read of the wire of its own.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let localCache, mountAgentLineage, rememberAgentLineageSupport, reader, changes;

const putRow = (entityId, row) =>
  localCache.writeCached({ deviceId: "dev-1", entityId, kind: "row" }, { run_id: entityId, project_id: "proj-1", ...row });

const mount = async (over = {}) => {
  reader = mountAgentLineage({ deviceId: "dev-1", projectId: "proj-1", onChanged: () => changes++, ...over });
  reader.start();
  await vi.waitFor(() => expect(changes).toBeGreaterThan(0));
  return reader;
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  changes = 0;
  localCache = await import("../src/core/localCache.js");
  ({ mountAgentLineage } = await import("../src/core/agentLineage.js"));
  ({ rememberAgentLineageSupport } = await import("../src/core/agentLineageSupport.js"));
  // A bridge that announces `agents.createdBy` greeted this device once.
  await rememberAgentLineageSupport("dev-1", { agents: { createdBy: true } });
});

afterEach(() => {
  reader?.dispose();
  reader = null;
});

describe("the Build agents it supplies", () => {
  it("reads nothing until it is started", async () => {
    await putRow("run-a", { kind: "workspace", workspace_id: "ws-a", agents: [{ id: "worker", created_by: "boss" }] });
    reader = mountAgentLineage({ deviceId: "dev-1", projectId: "proj-1", onChanged: () => changes++ });
    await new Promise((settled) => setTimeout(settled, 50));
    expect(changes).toBe(0);
    expect(reader.buildAgentsFor("boss")).toBeNull();
    reader.start();
    await vi.waitFor(() => expect(reader.buildAgentsFor("boss")).toHaveLength(1));
  });

  it("reads the agents a cached row names as made by this one", async () => {
    await putRow("run-boss", { kind: "project", agents: [{ id: "boss" }] });
    await putRow("run-a", { kind: "workspace", workspace_id: "ws-a", title: "Fix login", agents: [{ id: "worker", name: "Login fixer", created_by: "boss" }] });
    await mount();
    expect(reader.buildAgentsFor("boss")).toEqual([expect.objectContaining({
      id: "worker", name: "Login fixer", entity_id: "run-a", workspace_id: "ws-a", workspace_name: "Fix login",
    })]);
    expect(reader.buildAgentsFor("worker")).toBeNull();
  });

  it("supplies again when a row it holds is rewritten, and counts the creator running", async () => {
    await putRow("run-boss", { kind: "project", agents: [{ id: "boss" }] });
    await putRow("run-a", { kind: "workspace", workspace_id: "ws-a", agents: [{ id: "worker", created_by: "boss" }] });
    await mount();
    expect(reader.decorate({ id: "boss" }).agents_running).toBeUndefined();

    const before = changes;
    await putRow("run-a", { kind: "workspace", workspace_id: "ws-a", agents: [{ id: "worker", created_by: "boss", working: true }] });
    await vi.waitFor(() => expect(changes).toBeGreaterThan(before));
    expect(reader.buildAgentsFor("boss")[0].state).toBe("running");
    expect(reader.decorate({ id: "boss" }).agents_running).toBe(1);
  });

  it("drops a Build agent whose row no longer carries it", async () => {
    await putRow("run-a", { kind: "workspace", workspace_id: "ws-a", agents: [{ id: "worker", created_by: "boss" }] });
    await mount();
    const before = changes;
    await putRow("run-a", { kind: "workspace", workspace_id: "ws-a", agents: [] });
    await vi.waitFor(() => expect(changes).toBeGreaterThan(before));
    expect(reader.buildAgentsFor("boss")).toBeNull();
  });

  // A row is rewritten on every pull and push, mostly for things that are no
  // business of the lineage (a message, a read cursor). The rail repaints on
  // `onChanged`, so it hears only of what moved who made whom or what runs.
  it("says nothing when a rewritten row moved nothing it answers", async () => {
    await putRow("run-a", { kind: "workspace", workspace_id: "ws-a", agents: [{ id: "worker", created_by: "boss" }] });
    await mount();
    const before = changes;
    await putRow("run-a", { kind: "workspace", workspace_id: "ws-a", agents: [{ id: "worker", created_by: "boss", unread_count: 3 }] });
    // Long enough for the read that write set off to land.
    await new Promise((settled) => setTimeout(settled, 100));
    expect(changes).toBe(before);
    await putRow("run-b", { kind: "workspace", workspace_id: "ws-b", agents: [{ id: "worker-b", created_by: "boss", working: true }] });
    await vi.waitFor(() => expect(reader.buildAgentsFor("boss")).toHaveLength(2));
    expect(changes).toBe(before + 1);
  });

  it("reads another project's rows for nothing", async () => {
    await putRow("run-x", { kind: "workspace", project_id: "proj-2", agents: [{ id: "stranger", created_by: "boss" }] });
    await mount();
    expect(reader.buildAgentsFor("boss")).toBeNull();
  });

  it("lays its own agents' sub-agents into the rollup too", async () => {
    await putRow("run-a", { kind: "workspace", workspace_id: "ws-a", agents: [
      { id: "solo", surfaces: { subagents: [{ id: "s1", state: "running", started_at: 1000 }] } },
    ] });
    await mount();
    expect(reader.decorate({ id: "solo" }).agents_running).toBe(1);
  });

  it("supplies nothing, and reads nothing, without a device and a project", () => {
    const unnamed = mountAgentLineage({ deviceId: "", projectId: "proj-1", onChanged: () => changes++ });
    unnamed.start();
    expect(unnamed.buildAgentsFor("boss")).toBeNull();
    const agent = { id: "boss" };
    expect(unnamed.decorate(agent)).toBe(agent);
    unnamed.dispose();
  });
});

// #221: whether a machine's agents name their makers is the bridge's
// `agents.createdBy` capability, read from the cache the greeting wrote —
// not guessed from whether some row happens to carry `created_by`.
describe("gated on agents.createdBy", () => {
  it("lists no Build agents for a machine whose bridge never announced it", async () => {
    await rememberAgentLineageSupport("dev-1", { agents: { createdBy: false } });
    await putRow("run-a", { kind: "workspace", workspace_id: "ws-a", agents: [
      { id: "worker", created_by: "boss", working: true },
      { id: "solo", surfaces: { subagents: [{ id: "s1", state: "running", started_at: 1000 }] } },
    ] });
    await mount();
    expect(reader.buildAgentsFor("boss")).toBeNull();
    expect(reader.decorate({ id: "boss" }).agents_running).toBeUndefined();
    // The harness's own sub-agents are no Build agents: they still roll up.
    expect(reader.decorate({ id: "solo" }).agents_running).toBe(1);
  });

  it("lists them once a greeting says the bridge names makers", async () => {
    await rememberAgentLineageSupport("dev-1", { agents: { createdBy: false } });
    await putRow("run-a", { kind: "workspace", workspace_id: "ws-a", agents: [{ id: "worker", created_by: "boss" }] });
    await mount();
    expect(reader.buildAgentsFor("boss")).toBeNull();
    await rememberAgentLineageSupport("dev-1", { agents: { createdBy: true } });
    await vi.waitFor(() => expect(reader.buildAgentsFor("boss")).toHaveLength(1));
  });
});

// #226: the rows the bridge writes are branch rows, a workspace's included.
// Which workspace or project owns a row's run is in the cached lists, so the
// reader joins those too — and hears them move.
describe("where its Build agents live", () => {
  const putList = (kind, value) => localCache.writeCached({ deviceId: "dev-1", entityId: "", kind }, value);
  const onBranchRow = (runId, agents) => putRow(runId, { kind: "branch", branch: `build/${runId}`, title: `goal ${runId}`, agents });

  it("names a workspace's agent by the workspace whose run its row is", async () => {
    await putList("projects", [{ project_id: "proj-1", name: "Build", entity_id: "run-boss" }]);
    await putList("workspaces", [{ id: "ws-a", project_id: "proj-1", name: "Skrift validation", entity_id: "run-a" }]);
    await onBranchRow("run-boss", [{ id: "boss" }, { id: "deputy", created_by: "boss" }]);
    await onBranchRow("run-a", [{ id: "worker", created_by: "boss" }]);
    await mount();
    const byId = Object.fromEntries(reader.buildAgentsFor("boss").map((entry) => [entry.id, entry]));
    expect(byId.deputy).toMatchObject({ kind: "project", workspace_id: null });
    expect(byId.worker).toMatchObject({ kind: "workspace", workspace_id: "ws-a", workspace_name: "Skrift validation" });
  });

  it("supplies again when the workspace list lands after the rows", async () => {
    await onBranchRow("run-a", [{ id: "worker", created_by: "boss" }]);
    await mount();
    expect(reader.buildAgentsFor("boss")[0].kind).toBe("branch");
    await putList("workspaces", [{ id: "ws-a", project_id: "proj-1", name: "Skrift validation", entity_id: "run-a" }]);
    await vi.waitFor(() => expect(reader.buildAgentsFor("boss")[0]).toMatchObject({ kind: "workspace", workspace_id: "ws-a" }));
  });
});
