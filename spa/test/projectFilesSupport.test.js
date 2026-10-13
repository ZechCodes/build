import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
let capabilities = { fs: { projectSources: false } };
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: () => capabilities }));
vi.mock("../src/core/deviceContexts.js", () => ({ whenGreeted: (context, dispatch) => context.whenGreeted(dispatch) }));
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { projectFilesRpc } = await import("../src/core/projectFilesRpc.js");
const { readProjectFilesSupport, rememberProjectFilesSupport } = await import("../src/core/projectFilesSupport.js");
const { wipeCache } = await import("../src/core/localCache.js");
beforeEach(async () => { capabilities = { fs: { projectSources: false } }; await wipeCache(); });
const context = () => ({ deviceId: "d", rpc: vi.fn(async () => ({ ok: true })), whenGreeted: async (dispatch) => ({ sent: dispatch() }) });

describe("project source filesystem support", () => {
  it.each([
    ["added primary", [{ id: "code", path: "/code" }], [{ id: "docs", path: "/docs" }, { id: "code", path: "/code" }]],
    ["removed primary", [{ id: "code", path: "/code" }, { id: "docs", path: "/docs" }], [{ id: "docs", path: "/docs" }]],
    ["relabelled primary", [{ id: "code", path: "/code", name: "Code" }], [{ id: "code", path: "/code", name: "Renamed code" }]],
    ["moved primary", [{ id: "code", path: "/code" }], [{ id: "code", path: "/new-code" }]],
    ["cold project", null, [{ id: "docs", path: "/docs" }]],
  ])("reads the reconciled %s through a legacy bridge", async (_name, sources, nextSources) => {
    const machine = context();
    const opened = sources ? { sources } : null;
    let current = opened;
    const rpc = projectFilesRpc(machine, sources?.[0]?.id, { project: opened, currentProject: () => current });
    current = { sources: nextSources };
    const primary = nextSources[0];
    await rpc("fs.read", { project_id: "p", source_id: primary.id, source_path: primary.path, path: "README.md" });
    expect(machine.rpc).toHaveBeenCalledExactlyOnceWith("fs.read", { project_id: "p", path: "README.md" });
  });
  it("resolves the legacy primary again after a queued greeting", async () => {
    const machine = context();
    const before = { sources: [{ id: "code", path: "/code" }, { id: "docs", path: "/docs" }] };
    let current = before;
    machine.whenGreeted = async (dispatch) => {
      current = { sources: [{ id: "docs", path: "/docs" }] };
      return { sent: dispatch() };
    };
    const rpc = projectFilesRpc(machine, "code", { project: before, currentProject: () => current });
    await rpc("fs.read", { project_id: "p", source_id: "docs", source_path: "/docs", path: "README.md" });
    expect(machine.rpc).toHaveBeenCalledExactlyOnceWith("fs.read", { project_id: "p", path: "README.md" });
  });
  it("refuses stale locations and secondary sources on a reconciled legacy bridge", async () => {
    const machine = context();
    const before = { sources: [{ id: "code", path: "/code" }, { id: "docs", path: "/docs" }] };
    let current = { sources: [{ id: "docs", path: "/new-docs" }, { id: "other", path: "/other" }] };
    const rpc = projectFilesRpc(machine, "code", { project: before, currentProject: () => current });
    for (const method of ["fs.read", "fs.write"]) {
      await expect(rpc(method, { project_id: "p", source_id: "code", source_path: "/code", path: "README.md" })).rejects.toThrow("folder moved");
      await expect(rpc(method, { project_id: "p", source_id: "docs", source_path: "/docs", path: "README.md" })).rejects.toThrow("folder moved");
    }
    await expect(rpc("fs.read", { project_id: "p", source_id: "other", source_path: "/other", path: "README.md" })).rejects.toThrow("Update the bridge");
    current = { sources: [] };
    await expect(rpc("fs.write", { project_id: "p", source_id: "docs", source_path: "/new-docs", path: "README.md" })).rejects.toThrow("folder moved");
    expect(machine.rpc).not.toHaveBeenCalled();
  });
  it("addresses upload sessions by id even after their project source moved", async () => {
    const machine = context();
    const before = { path: "/old", sources: [{ id: "docs", path: "/old" }] };
    const after = { path: "/new", sources: [{ id: "docs", path: "/new" }] };
    const rpc = projectFilesRpc(machine, "code", { project: before, currentProject: () => after });
    for (const method of ["fs.uploadChunk", "fs.uploadFinish", "fs.uploadAbort"]) {
      await rpc(method, { upload_id: "u" });
      expect(machine.rpc).toHaveBeenCalledWith(method, { upload_id: "u" });
    }
  });
  it("refuses reading and saving a mounted source after its folder moved", async () => {
    const machine = context();
    capabilities.fs.projectSources = true;
    const before = { sources: [{ id: "docs", path: "/old" }] };
    const after = { sources: [{ id: "docs", path: "/new" }] };
    const rpc = projectFilesRpc(machine, "docs", { project: before, currentProject: () => after });
    for (const method of ["fs.tree", "fs.read", "fs.write"]) {
      await expect(rpc(method, { project_id: "p", source_id: "docs", path: "README.md" })).rejects.toThrow("folder moved");
    }
    expect(machine.rpc).not.toHaveBeenCalled();
  });
  it("names the folder it opened so the bridge itself refuses a source that moved", async () => {
    const machine = context();
    capabilities.fs = { projectSources: true, projectSourcePath: true };
    const project = { sources: [{ id: "docs", path: "/docs" }] };
    const rpc = projectFilesRpc(machine, "code", { project, currentProject: () => project });
    for (const method of ["fs.tree", "fs.read", "fs.write", "fs.createDirectory", "fs.createFile", "fs.uploadBegin"]) {
      await rpc(method, { project_id: "p", source_id: "docs", path: "README.md" });
      expect(machine.rpc).toHaveBeenLastCalledWith(method, { project_id: "p", source_id: "docs", path: "README.md", source_path: "/docs" });
    }
  });
  it("sends no source path to a bridge that does not announce it", async () => {
    const machine = context();
    capabilities.fs.projectSources = true;
    const project = { sources: [{ id: "docs", path: "/docs" }] };
    await projectFilesRpc(machine, "code", { project, currentProject: () => project })("fs.write", { project_id: "p", source_id: "docs", path: "README.md" });
    expect(machine.rpc).toHaveBeenCalledWith("fs.write", { project_id: "p", source_id: "docs", path: "README.md" });
  });
  it("reads a newly added or moved root from its captured location after reconciliation", async () => {
    const machine = context();
    capabilities.fs = { projectSources: true, projectSourcePath: true };
    const before = { sources: [{ id: "code", path: "/old" }] };
    const after = { sources: [{ id: "code", path: "/new" }, { id: "docs", path: "/docs" }] };
    const rpc = projectFilesRpc(machine, "code", { project: before, currentProject: () => after });
    for (const [source_id, source_path] of [["code", "/new"], ["docs", "/docs"]]) {
      const params = { project_id: "p", source_id, source_path, path: "README.md" };
      await rpc("fs.read", params);
      expect(machine.rpc).toHaveBeenLastCalledWith("fs.read", params);
    }
  });
  it("refuses a captured location after move or removal but permits other source drafts", async () => {
    const machine = context();
    capabilities.fs = { projectSources: true, projectSourcePath: true };
    const before = { sources: [{ id: "code", path: "/code" }, { id: "docs", path: "/docs" }] };
    let current = { sources: [{ id: "code", path: "/moved" }, { id: "docs", path: "/docs" }] };
    const rpc = projectFilesRpc(machine, "code", { project: before, currentProject: () => current });
    const stale = { project_id: "p", source_id: "code", source_path: "/code", path: "README.md" };
    for (const method of ["fs.read", "fs.write"]) await expect(rpc(method, stale)).rejects.toThrow("folder moved");
    current = { sources: [{ id: "docs", path: "/docs" }] };
    await expect(rpc("fs.write", stale)).rejects.toThrow("folder moved");
    const safe = { project_id: "p", source_id: "docs", source_path: "/docs", path: "draft.md", content: "draft" };
    await rpc("fs.write", safe);
    expect(machine.rpc).toHaveBeenCalledExactlyOnceWith("fs.write", safe);
  });
  it("removes captured source paths before calling an older source-aware bridge", async () => {
    const machine = context();
    capabilities.fs.projectSources = true;
    const project = { sources: [{ id: "docs", path: "/docs" }] };
    const params = { project_id: "p", source_id: "docs", source_path: "/docs", path: "README.md" };
    await projectFilesRpc(machine, "code", { project, currentProject: () => project })("fs.read", params);
    expect(machine.rpc).toHaveBeenCalledWith("fs.read", { project_id: "p", source_id: "docs", path: "README.md" });
    expect(params.source_path).toBe("/docs");
  });
  it("removes both selectors before calling a legacy primary bridge", async () => {
    const machine = context();
    const project = { sources: [{ id: "code", path: "/code" }] };
    await projectFilesRpc(machine, "code", { project, currentProject: () => project })("fs.write", { project_id: "p", source_id: "code", source_path: "/code", path: "README.md" });
    expect(machine.rpc).toHaveBeenCalledWith("fs.write", { project_id: "p", path: "README.md" });
  });
  it("keeps rendering support in the device cache for a cold mount", async () => {
    await rememberProjectFilesSupport("d", { fs: { projectSources: true } });
    expect(await readProjectFilesSupport("d")).toBe(true);
    expect(await readProjectFilesSupport("other")).toBe(false);
    await rememberProjectFilesSupport("d", { fs: { projectSources: false } });
    expect(await readProjectFilesSupport("d")).toBe(false);
  });
  it("refuses an additional source before asking a legacy bridge", async () => {
    const machine = context();
    await expect(projectFilesRpc(machine, "code")("fs.read", { project_id: "p", source_id: "docs", path: "README.md" })).rejects.toThrow("Update the bridge");
    expect(machine.rpc).not.toHaveBeenCalled();
  });
  it("lets a legacy bridge read the primary without an unknown source field", async () => {
    const machine = context();
    await projectFilesRpc(machine, "code")("fs.read", { project_id: "p", source_id: "code", path: "README.md" }, { priority: "background" });
    expect(machine.rpc).toHaveBeenCalledWith("fs.read", { project_id: "p", path: "README.md" }, { priority: "background" });
  });
  it("checks current support after the greeting and preserves read options", async () => {
    const machine = context();
    machine.whenGreeted = async (dispatch) => { capabilities.fs.projectSources = true; return { sent: dispatch() }; };
    const params = { project_id: "p", source_id: "docs", path: "README.md" };
    await projectFilesRpc(machine, "code")("fs.read", params, { priority: "background" });
    expect(machine.rpc).toHaveBeenCalledWith("fs.read", params, { priority: "background" });
  });
});
