import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
let capabilities;
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: () => capabilities }));
vi.mock("../src/core/deviceContexts.js", () => ({ whenGreeted: (context, dispatch) => context.whenGreeted(dispatch) }));
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { projectChangesDirectories, projectSourceContext } = await import("../src/core/projectChangesModel.js");
const { projectChangesRpc } = await import("../src/core/projectChangesRpc.js");
const { readProjectChangesSupport, rememberProjectChangesSupport } = await import("../src/core/projectChangesSupport.js");
const { directoryCacheId } = await import("../src/core/directoryScope.js");
const { wipeCache } = await import("../src/core/localCache.js");
const project = { id: "p", sources: [
  { id: "code", name: "Code", path: "/code", is_git: true, base_branch: "main", sync_base: true },
  { id: "docs", name: "Docs", path: "/docs", is_git: true, base_branch: "stable", sync_base: false },
  { id: "assets", name: "Assets", path: "/assets", is_git: false },
] };
const context = () => ({ deviceId: "d", rpc: vi.fn(async () => ({ ok: true })), whenGreeted: async (dispatch) => ({ sent: dispatch() }) });
beforeEach(async () => { capabilities = { git: { projectSources: true }, projects: { syncBase: true, syncBasePath: true } }; await wipeCache(); });
it("models every registered source with captured location, including plain folders", () => {
  const dirs = projectChangesDirectories(project, "p", true);
  expect(dirs.map((d) => d.sourceId)).toEqual(["code", "docs", "assets"]);
  expect(dirs[1]).toMatchObject({ label: "Docs", scope: { project_id: "p", source_id: "docs", source_path: "/docs" } });
  expect(dirs[2].is_git).toBe(false);
});
it("keeps legacy primary readable and additional sources visibly unavailable", () => {
  const dirs = projectChangesDirectories(project, "p", false);
  expect(dirs[0].scope).toEqual({ project_id: "p" });
  expect(dirs[1]).toMatchObject({ status: "unsupported" });
  expect(dirs[1].error).toMatch(/Update/);
  expect(dirs[2].is_git).toBe(false);
});
it("namespaces source records by source and captured folder", () => {
  const scope = { project_id: "p", source_id: "code", source_path: "/code" };
  expect(directoryCacheId(scope)).toBe('project:["p","code","/code"]');
  expect(directoryCacheId(scope)).not.toBe(directoryCacheId({ ...scope, source_path: "/moved" }));
  expect(directoryCacheId(scope)).not.toBe(directoryCacheId({ ...scope, source_id: "docs" }));
});
it("shows configured base and its cached sync outcome independently of HEAD", () => {
  const source = { ...project.sources[0], sync: { state: "skipped", reason: "base has local commits", last_attempt_ms: 1 } };
  const text = projectSourceContext(source, 10);
  expect(text).toMatch(/Base main/);
  expect(text).toMatch(/local commits/);
  expect(projectSourceContext(project.sources[1])).toMatch(/Automatic base sync off/);
});
it("persists support for a cold cache mount and updates after downgrade", async () => {
  expect(await readProjectChangesSupport("d")).toEqual({ projectSources: false, syncBase: false, syncBasePath: false });
  await rememberProjectChangesSupport("d", capabilities);
  expect(await readProjectChangesSupport("d")).toEqual({ projectSources: true, syncBase: true, syncBasePath: true });
  await rememberProjectChangesSupport("d", {});
  expect((await readProjectChangesSupport("d")).projectSources).toBe(false);
});
it.each(["git.stage", "git.commit", "git.discard", "git.checkout_ref", "git.status", "project.sync_source"])("refuses stale %s before it reaches a replacement source", async (method) => {
  const machine = context();
  let current = project;
  const rpc = projectChangesRpc(machine, project.sources[1], { projectId: "p", currentProject: () => current });
  current = { ...project, sources: project.sources.map((s) => s.id === "docs" ? { ...s, path: "/moved" } : s) };
  await expect(rpc(method, { project_id: "p", source_id: "docs", source_path: "/docs" })).rejects.toThrow(/moved|removed/);
  expect(machine.rpc).not.toHaveBeenCalled();
});
it("dispatches full source scope for history and Sync base", async () => {
  const machine = context();
  const rpc = projectChangesRpc(machine, project.sources[1], { projectId: "p", currentProject: () => project });
  await rpc("git.log", { project_id: "p", source_id: "docs" });
  expect(machine.rpc).toHaveBeenCalledWith("git.log", { project_id: "p", source_id: "docs", source_path: "/docs" });
  await rpc("project.sync_source", { project_id: "p", source_id: "docs" });
  expect(machine.rpc).toHaveBeenLastCalledWith("project.sync_source", { project_id: "p", source_id: "docs", source_path: "/docs" });
});
it("never sends new selectors to a legacy bridge, refusing secondary and moved primary", async () => {
  capabilities = { git: { projectSources: false }, projects: {} };
  const machine = context();
  const rpc = projectChangesRpc(machine, project.sources[0], { projectId: "p", currentProject: () => project });
  await rpc("git.status", { project_id: "p", source_id: "code", source_path: "/code" });
  expect(machine.rpc).toHaveBeenLastCalledWith("git.status", { project_id: "p" });
  const other = projectChangesRpc(machine, project.sources[1], { projectId: "p", currentProject: () => project });
  await expect(other("git.commit", { project_id: "p", source_id: "docs" })).rejects.toThrow(/Update/);
});
it("rechecks the source and capabilities when a queued greeting dispatches", async () => {
  const machine = context();
  let dispatch;
  machine.whenGreeted = vi.fn(async (fn) => { dispatch = fn; return { sent: Promise.resolve() }; });
  let current = project;
  const rpc = projectChangesRpc(machine, project.sources[0], { projectId: "p", currentProject: () => current });
  await rpc("git.commit", { project_id: "p", source_id: "code" });
  current = { ...project, sources: [] };
  expect(() => dispatch()).toThrow(/removed/);
});
