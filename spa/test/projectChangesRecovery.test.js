/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const mountWorkspaceChanges = vi.fn((host, options) => ({
  workspaceMoved: vi.fn(), selectDirectory: vi.fn((id) => options.onSelectDirectory(id)), setVisible() {}, dispose() {},
}));
vi.mock("../src/views/workspaceChanges.js", () => ({ mountWorkspaceChanges: (...args) => mountWorkspaceChanges(...args) }));
vi.mock("../src/core/deviceContexts.js", async importOriginal => ({
  ...await importOriginal(),
  captureDeviceGreeting: () => ({ epoch: "changes-epoch", current: () => true }),
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch() }),
}));
vi.mock("../src/core/changeEvents.js", async importOriginal => ({
  ...await importOriginal(), bridgeCapabilities: () => ({ projects: { syncBase: true }, git: { projectSources: true } }),
}));
const { mountProjectChanges } = await import("../src/views/projectChanges.js");
const { rememberProjectChangesSupport } = await import("../src/core/projectChangesSupport.js");
const { writeUiRecord, wipeUiRecords } = await import("../src/core/localUiStore.js");
const { App } = await import("../src/appState.js");
const { directoryCacheId } = await import("../src/core/directoryScope.js");
const { writeCached, wipeCache } = await import("../src/core/localCache.js");
const { environmentSupportAddress } = await import("../src/core/environmentSupport.js");
const project = { id: "p", sources: [{ id: "code", name: "Code", path: "/code", is_git: true }] };
const draftAddress = (deviceId, projectId, sourceId, path) => {
  const entityId = directoryCacheId({ project_id: projectId, source_id: sourceId, source_path: path });
  return { deviceId, entityId, kind: "ui-draft", sub: entityId };
};
let handle;
let host;
const changesRpc = vi.fn(async () => ({}));
const mount = (initial = project, route = {}) => {
  host = document.createElement("div");
  document.body.append(host);
  const onSourceSelection = vi.fn();
  handle = mountProjectChanges(host, { project: initial, route: { projectId: "p", ...route },
    context: { deviceId: "d", rpc: changesRpc, cacheScope: { deviceId: "d", address: (address) => ({ deviceId: "d", ...address }) } },
    onSourceSelection, onCommitSelection() {} });
  return onSourceSelection;
};
beforeEach(async () => {
  mountWorkspaceChanges.mockClear();
  changesRpc.mockClear();
  await wipeCache();
  await wipeUiRecords();
  await rememberProjectChangesSupport("d", { git: { projectSources: true } });
});
afterEach(() => { handle?.dispose(); document.body.replaceChildren(); });

it.each(["inherited", "unknown", "added", "legacy"])("protects the Sync base affordance and handler for %s sources", async kind => {
  await rememberProjectChangesSupport("d", { git: { projectSources: true }, projects: { syncBase: true } });
  await writeCached(environmentSupportAddress("d"), {
    list: false, createProject: false, create: false, addedSources: false, archive: false, rename: false, secrets: false,
    bridge_epoch: kind === "unknown" ? "pre-upgrade-epoch" : "changes-epoch",
  });
  const current = { ...project };
  if (kind === "inherited") { current.environment_id = "env"; current.added_sources = []; }
  if (kind === "added") current.added_sources = project.sources;
  mount(current);
  await vi.waitFor(() => expect(mountWorkspaceChanges).toHaveBeenCalled());
  const sync = host.querySelector("[data-project-sync-base]");
  const allowed = ["added", "legacy"].includes(kind);
  expect(sync.hidden).toBe(!allowed);
  sync.click();
  if (allowed) await vi.waitFor(() => expect(changesRpc).toHaveBeenCalledWith("project.sync_source", expect.objectContaining({ project_id: "p", source_id: "code" })));
  else {
    await vi.waitFor(() => expect(host.querySelector("[data-project-sync-error]").textContent).toMatch(/read-only|ownership/i));
    expect(changesRpc).not.toHaveBeenCalled();
  }
});

it("preserves a commit-only deep link until the default source resolves", async () => {
  const selected = mount({ id: "p", sources: [] }, { commit: "abc" });
  await vi.waitFor(() => expect(mountWorkspaceChanges).toHaveBeenCalled());
  handle.update(project);
  const options = mountWorkspaceChanges.mock.calls[0][1];
  expect(options.git("code").requestedCommit).toBe("abc");
  expect(selected).toHaveBeenLastCalledWith("code", "abc");
});

it("mounts surviving sources for a fresh removed-source bookmark and visibly offers recovery", async () => {
  const selected = mount(project, { sourceId: "docs", commit: "abc" });
  await vi.waitFor(() => expect(mountWorkspaceChanges).toHaveBeenCalled());
  const options = mountWorkspaceChanges.mock.calls[0][1];
  expect(options.directories.map((directory) => directory.sourceId)).toEqual(["code"]);
  expect(options.current).toBe("code");
  expect(options.git("code").requestedCommit).toBeNull();
  expect(host.querySelector("[data-project-missing-source]").textContent).toMatch(/docs.*unavailable/i);
  expect(selected).not.toHaveBeenCalled();
  handle.update({ ...project, name: "Updated project" });
  expect(mountWorkspaceChanges).toHaveBeenCalledTimes(1);
  expect(options.git("code").requestedCommit).toBeNull();
  host.querySelector("[data-project-use-source]").click();
  expect(selected).toHaveBeenLastCalledWith("code", null);
  expect(host.querySelector("[data-project-missing-source]").hidden).toBe(true);
});

it("reconciles away the last survivor while keeping an unresolved bookmark pending", async () => {
  const selected = mount(project, { sourceId: "docs", commit: "abc" });
  await vi.waitFor(() => expect(mountWorkspaceChanges).toHaveBeenCalled());
  const pane = mountWorkspaceChanges.mock.results[0].value;
  handle.update({ ...project, sources: [] });
  expect(pane.workspaceMoved).toHaveBeenLastCalledWith([]);
  expect(selected).not.toHaveBeenCalled();
  expect(host.querySelector("[data-project-use-source]").hidden).toBe(true);
});

it("selects the exact pending source even when reconciliation first promotes another survivor", async () => {
  const selected = mount(project, { sourceId: "docs", commit: "abc" });
  await vi.waitFor(() => expect(mountWorkspaceChanges).toHaveBeenCalled());
  const options = mountWorkspaceChanges.mock.calls[0][1];
  const pane = mountWorkspaceChanges.mock.results[0].value;
  pane.workspaceMoved.mockImplementation((directories) => options.onSelectDirectory(directories[0].sourceId));
  handle.update({ ...project, sources: [
    { id: "assets", path: "/assets", is_git: false },
    { id: "docs", path: "/docs", is_git: true },
  ] });
  expect(pane.selectDirectory).toHaveBeenLastCalledWith("docs");
  expect(selected).toHaveBeenLastCalledWith("docs", "abc");
  expect(selected.mock.calls).toEqual([["docs", "abc"]]);
  expect(options.git("docs").requestedCommit).toBe("abc");
});

it.each([null, { id: "p", sources: [] }, project, { id: "p", sources: [{ id: "docs", is_git: true }] }])("keeps an explicit source and commit pending until their source location resolves (%j)", async (initial) => {
  const selected = mount(initial, { sourceId: "docs", commit: "abc" });
  await vi.waitFor(() => expect(host.querySelector("[data-project-source-context]").textContent).not.toBe(""));
  expect(selected).not.toHaveBeenCalled();
  const resolved = { ...project, sources: [...project.sources, { id: "docs", name: "Docs", path: "/docs", is_git: true }] };
  handle.update(resolved);
  await vi.waitFor(() => expect(mountWorkspaceChanges).toHaveBeenCalled());
  const options = mountWorkspaceChanges.mock.calls.at(-1)[1];
  if (initial === project) expect(mountWorkspaceChanges.mock.results.at(-1).value.selectDirectory).toHaveBeenCalledWith("docs");
  else expect(options.current).toBe("docs");
  expect(options.git("docs").requestedCommit).toBe("abc");
  expect(selected).toHaveBeenLastCalledWith("docs", "abc");
  handle.update({ ...resolved, name: "Relabelled project" });
  expect(options.git("docs").requestedCommit).toBe("abc");
  handle.update(project);
  expect(selected).toHaveBeenLastCalledWith("code", null);
});

it("visibly recovers persisted moved and removed source drafts without retargeting them", async () => {
  await writeUiRecord(draftAddress("d", "p", "code", "/old-code"), { text: "Moved draft" });
  await writeUiRecord(draftAddress("d", "p", "docs", "/docs"), { text: "Removed draft" });
  await writeUiRecord(draftAddress("d", "p", "code", "/code"), { text: "Live draft" });
  await writeUiRecord(draftAddress("other-device", "p", "docs", "/docs"), { text: "Foreign device" });
  await writeUiRecord(draftAddress("d", "other-project", "docs", "/docs"), { text: "Foreign project" });
  mount();
  await vi.waitFor(() => expect(host.querySelectorAll("[data-recovered-draft]")).toHaveLength(2));
  const fields = [...host.querySelectorAll("[data-recovered-draft] textarea")];
  expect(fields.map((field) => field.value)).toEqual(["Moved draft", "Removed draft"]);
  expect(fields.every((field) => field.readOnly)).toBe(true);
  expect(host.textContent).toContain("/old-code");
  expect(host.textContent).toContain("removed");
  expect(host.textContent).not.toContain("Foreign");
});

it("discovers drafts flushed during reconciliation and removes recovered rows when their location returns", async () => {
  mount();
  await vi.waitFor(() => expect(mountWorkspaceChanges).toHaveBeenCalled());
  handle.update({ ...project, sources: [] });
  await writeUiRecord(draftAddress("d", "p", "code", "/code"), { text: "Last keystroke" });
  await vi.waitFor(() => expect(host.querySelector("[data-recovered-draft] textarea")?.value).toBe("Last keystroke"));
  handle.update(project);
  await vi.waitFor(() => expect(host.querySelector("[data-recovered-draft]")).toBeNull());
});

it("ignores UI store readbacks from a replaced account", async () => {
  await writeUiRecord(draftAddress("d", "p", "docs", "/docs"), { text: "Old account draft" });
  mount();
  App.accountEpoch += 1;
  await new Promise((resolve) => setTimeout(resolve, 40));
  expect(host.querySelector("[data-recovered-draft]")).toBeNull();
});
