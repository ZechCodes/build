// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const ownership = vi.hoisted(() => ({ owns: false, wake: vi.fn(async () => []) }));
vi.mock("../src/core/scopedSyncRuntime.js", () => ({
  deviceScopedSyncOwns: () => ownership.owns,
  wakeScopedSync: (...args) => ownership.wake(...args),
}));
vi.mock("../src/app.js", () => ({ App: { devices: [] } }));
const { readCached, wipeCache, writeCached } = await import("../src/core/localCache.js");
const { deviceProjectsAddress, projectSettingsAddress } = await import("../src/core/settingsRecords.js");
const { deviceProjectsPanelHtml, mountDeviceProjects } = await import("../src/views/deviceProjects.js");
const { openProjectSettings } = await import("../src/sheets/projectSettings.js");
const { openWorkspaceSettings } = await import("../src/sheets/workspaceSettings.js");
const project = { project_id: "directory-project", name: "Engine project", path: "/synthetic/project", base_branch: "main", sources: [] };
const listAddress = deviceProjectsAddress("directory-device"), detailAddress = projectSettingsAddress("directory-device", project.project_id);

beforeEach(async () => {
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  await wipeCache(); ownership.owns = false; ownership.wake.mockClear();
  await writeCached(listAddress, [project]); await writeCached(detailAddress, project);
  document.body.innerHTML = `${deviceProjectsPanelHtml()}<div id="scrim"><div id="sheet"></div></div>`;
});
afterEach(() => { document.body.innerHTML = ""; });

it.each([false, true])("uses engine directory reads only for the exact owned device scope (owned=%s)", async (owns) => {
  ownership.owns = owns;
  const callRpc = vi.fn(async () => ({ projects: [project] }));
  await mountDeviceProjects(document.body, { deviceId: "directory-device", callRpc });
  expect(callRpc.mock.calls.filter(([method]) => method === "project.list").length).toBe(Number(!owns));
  expect(document.querySelector("#projlist").textContent).toContain("Engine project");
  expect(ownership.wake).toHaveBeenCalledTimes(Number(owns));
});

it("rejects a mounted legacy directory reply and detail copy after capability upgrades", async () => {
  let finish;
  const callRpc = vi.fn(() => new Promise((resolve) => { finish = resolve; }));
  const mounted = mountDeviceProjects(document.body, { deviceId: "directory-device", callRpc });
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  ownership.owns = true;
  finish({ projects: [{ ...project, name: "Late legacy project" }] });
  await mounted;
  expect((await readCached(listAddress)).value).toEqual([project]);
  expect((await readCached(detailAddress)).value).toEqual(project);
  expect(document.querySelector("#projlist").textContent).toContain("Engine project");
});

it("opens project settings from the engine cache without an autonomous legacy list read", async () => {
  ownership.owns = true;
  const callRpc = vi.fn(async () => ({ projects: [project] }));
  const mounted = openProjectSettings(project.project_id, { deviceId: "directory-device", callRpc });
  await mounted.whenCachePainted();
  expect(document.querySelector("#psproject").value).toBe("Engine project");
  expect(callRpc.mock.calls.filter(([method]) => method === "project.list")).toEqual([]);
});


it("offers project sources in workspace settings from the engine cache", async () => {
  ownership.owns = true;
  const detail = { workspace_id: "directory-workspace", project_id: project.project_id, name: "Workspace", directories: [] };
  const callRpc = vi.fn(async (method) => method === "project.list" ? { projects: [project] } : detail);
  const close = openWorkspaceSettings({ id: detail.workspace_id, name: detail.name }, {
    deviceId: "directory-device", callRpc, catalog: { providers: [], agent_modes: {} },
  });
  try {
    await vi.waitFor(() => expect(document.querySelector("#wsdiradd")).toBeTruthy());
    expect(callRpc.mock.calls.filter(([method]) => method === "project.list")).toEqual([]);
  } finally { await close(); }
});
