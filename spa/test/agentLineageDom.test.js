/** @vitest-environment jsdom */
// The lineage reader (#216): who made whom, read off the device's cached rows
// and nothing else. A pull or a push rewrites a row record; the reader hears
// the record and supplies again, with no read of the wire of its own.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let localCache, mountAgentLineage, reader, changes;

const putRow = (entityId, row) =>
  localCache.writeCached({ deviceId: "dev-1", entityId, kind: "row" }, { run_id: entityId, project_id: "proj-1", ...row });

const mount = async (over = {}) => {
  reader = mountAgentLineage({ deviceId: "dev-1", projectId: "proj-1", onChanged: () => changes++, ...over });
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
});

afterEach(() => {
  reader?.dispose();
  reader = null;
});

describe("the Build agents it supplies", () => {
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
    expect(unnamed.buildAgentsFor("boss")).toBeNull();
    const agent = { id: "boss" };
    expect(unnamed.decorate(agent)).toBe(agent);
    unnamed.dispose();
  });
});
