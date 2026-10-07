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
