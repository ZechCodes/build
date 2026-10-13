/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { mountProjectChangesDrafts } = await import("../src/core/projectChangesDrafts.js");
const { writeUiRecord, readUiRecord, wipeUiRecords } = await import("../src/core/localUiStore.js");
const { rememberProjectChangesSupport } = await import("../src/core/projectChangesSupport.js");
const { App } = await import("../src/appState.js");
const project = { id: "p", sources: [{ id: "code", name: "Code", path: "/code", is_git: true }] };
const legacyAddress = { deviceId: "d", entityId: 'project:["p"]', kind: "ui-draft", sub: "project:p" };
let handle;
let host;
const mount = () => {
  host = document.createElement("div");
  document.body.append(host);
  handle = mountProjectChangesDrafts(host, { deviceId: "d", project });
};
beforeEach(async () => { await wipeUiRecords(); });
afterEach(() => { handle?.dispose(); document.body.replaceChildren(); });

it("surfaces a legacy draft after source support arrives without guessing its location", async () => {
  await rememberProjectChangesSupport("d", {});
  await writeUiRecord(legacyAddress, { text: "Draft from the old composer" });
  await rememberProjectChangesSupport("d", { git: { projectSources: true } });
  mount();
  await vi.waitFor(() => expect(host.querySelector("textarea")?.value).toBe("Draft from the old composer"));
  expect(host.querySelector("textarea").readOnly).toBe(true);
  expect(host.textContent).toMatch(/legacy/i);
  expect(host.textContent).toMatch(/unknown/i);
  expect(host.textContent).not.toContain("/code");
  handle.update({ ...project, sources: [{ ...project.sources[0], path: "/moved" }] });
  await vi.waitFor(() => expect(host.querySelector("textarea")?.value).toBe("Draft from the old composer"));
  expect((await readUiRecord(legacyAddress)).value.text).toBe("Draft from the old composer");
});

it("keeps legacy recovery fenced by device, project and account", async () => {
  await writeUiRecord({ ...legacyAddress, deviceId: "other" }, { text: "Foreign device" });
  await writeUiRecord({ ...legacyAddress, entityId: 'project:["other"]', sub: "project:other" }, { text: "Foreign project" });
  mount();
  await new Promise((resolve) => setTimeout(resolve, 40));
  expect(host.querySelector("textarea")).toBeNull();
  App.accountEpoch += 1;
  await writeUiRecord(legacyAddress, { text: "Replacement account" });
  await new Promise((resolve) => setTimeout(resolve, 40));
  expect(host.querySelector("textarea")).toBeNull();
});
