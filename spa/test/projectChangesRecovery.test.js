/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const mountWorkspaceChanges = vi.fn(() => ({ workspaceMoved() {}, setVisible() {}, dispose() {} }));
vi.mock("../src/views/workspaceChanges.js", () => ({ mountWorkspaceChanges: (...args) => mountWorkspaceChanges(...args) }));
const { mountProjectChanges } = await import("../src/views/projectChanges.js");
const { rememberProjectChangesSupport } = await import("../src/core/projectChangesSupport.js");
const { writeUiRecord, wipeUiRecords } = await import("../src/core/localUiStore.js");
const { App } = await import("../src/appState.js");
const { directoryCacheId } = await import("../src/core/directoryScope.js");
const project = { id: "p", sources: [{ id: "code", name: "Code", path: "/code", is_git: true }] };
const draftAddress = (deviceId, projectId, sourceId, path) => {
  const entityId = directoryCacheId({ project_id: projectId, source_id: sourceId, source_path: path });
  return { deviceId, entityId, kind: "ui-draft", sub: entityId };
};
let handle;
let host;
const mount = (initial = project, route = {}) => {
  host = document.createElement("div");
  document.body.append(host);
  const onSourceSelection = vi.fn();
  handle = mountProjectChanges(host, { project: initial, route: { projectId: "p", ...route },
    context: { deviceId: "d", cacheScope: { deviceId: "d", address: (address) => ({ deviceId: "d", ...address }) } },
    onSourceSelection, onCommitSelection() {} });
  return onSourceSelection;
};
beforeEach(async () => {
  mountWorkspaceChanges.mockClear();
  await wipeUiRecords();
  await rememberProjectChangesSupport("d", { git: { projectSources: true } });
});
afterEach(() => { handle?.dispose(); document.body.replaceChildren(); });

it("preserves a commit-only deep link until the default source resolves", async () => {
  const selected = mount({ id: "p", sources: [] }, { commit: "abc" });
  await vi.waitFor(() => expect(mountWorkspaceChanges).toHaveBeenCalled());
  handle.update(project);
  const options = mountWorkspaceChanges.mock.calls[0][1];
  expect(options.git("code").requestedCommit).toBe("abc");
  expect(selected).toHaveBeenLastCalledWith("code", "abc");
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
