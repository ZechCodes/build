/** @vitest-environment jsdom */
// Real greeting-to-cache-to-sheet wiring. Current project rows identify the
// inherited home and the added docs folder; pre-environment bridges keep the
// old row shape and its legacy edit controls.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { readSourceEditSupport } = await import("../src/core/sourceEditSupport.js");
const { openProjectSettings } = await import("../src/sheets/projectSettings.js");
const { wipeCache } = await import("../src/core/localCache.js");
const { wipeUiRecords } = await import("../src/core/localUiStore.js");

const greeting = JSON.parse(readFileSync(resolve(process.cwd(), "../fixtures/api/v1/session.hello.json"), "utf8")).result;
// Earlier bridges really lack the environment capability family. Removing
// only update_source from today's hello does not describe a legacy bridge.
const legacyGreeting = { ...greeting, api_version: "3.35.0", capabilities: greeting.capabilities.filter(name =>
  !name.startsWith("environment") && !name.startsWith("projects.") && name !== "secrets.environmentScope") };
const olderGreeting = { ...legacyGreeting, capabilities: legacyGreeting.capabilities.filter(name => name !== "project.update_source") };

const PROJECT = {
  project_id: "proj-1",
  name: "build",
  sources: [
    { id: "source-1", name: "build", mount: "build", path: "/b", is_git: true, base_branch: "main", remote: null },
    { id: "source-2", name: "docs", mount: "docs", path: "/d", is_git: true, base_branch: "main", remote: null },
  ],
};

// Current project.list rows identify inherited and added sources explicitly.
const CURRENT_PROJECT = { ...PROJECT, environment_id: "env-1", environment_name: "Build", added_sources: [PROJECT.sources[1]] };

beforeEach(async () => {
  resetChangeEvents();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  await wipeCache();
  await wipeUiRecords();
});

afterEach(() => resetChangeEvents());

const greet = (hello) => greetBridge(async (method) => (method === "session.hello" ? hello : {}), { deviceId: "dev-1", strict: true });
const docsLabel = () => document.querySelector('#sheet .ps-source[data-source-id="source-2"] input[data-field="name"]');

it("offers added-source edits once the bridge's real greeting names project.update_source", async () => {
  expect(greeting.capabilities).toContain("project.update_source");
  await greet(greeting);
  await vi.waitFor(async () => expect(await readSourceEditSupport("dev-1")).toBe(true));
  openProjectSettings("proj-1", { callRpc: vi.fn(async () => ({ projects: [CURRENT_PROJECT] })), deviceId: "dev-1" });
  await vi.waitFor(() => expect(docsLabel()?.readOnly).toBe(false));
  expect(document.querySelector('#sheet .ps-source[data-source-id="source-1"] input[data-field="name"]').readOnly).toBe(true);
});

it("keeps them read-only for a bridge whose greeting does not name it", async () => {
  await greet(olderGreeting);
  openProjectSettings("proj-1", { callRpc: vi.fn(async () => ({ projects: [PROJECT] })), deviceId: "dev-1" });
  await vi.waitFor(() => expect(docsLabel()).toBeTruthy());
  expect(await readSourceEditSupport("dev-1")).toBe(false);
  expect(docsLabel().readOnly).toBe(true);
});

it("offers keeping each source's base up to date once the real greeting names sources.syncBase (#267)", async () => {
  const { readSourceSyncSupport } = await import("../src/core/sourceEditSupport.js");
  expect(greeting.capabilities).toContain("sources.syncBase");
  await greet(greeting);
  await vi.waitFor(async () => expect(await readSourceSyncSupport("dev-1")).toBe(true));
  openProjectSettings("proj-1", { callRpc: vi.fn(async () => ({ projects: [CURRENT_PROJECT] })), deviceId: "dev-1" });
  await vi.waitFor(() => expect(document.querySelector('#sheet .ps-source[data-source-id="source-2"] [data-sync-base]')).toBeTruthy());
});

it("offers no sync controls to a bridge whose greeting does not name sources.syncBase", async () => {
  const { readSourceSyncSupport } = await import("../src/core/sourceEditSupport.js");
  await greet({ ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "sources.syncBase") });
  openProjectSettings("proj-1", { callRpc: vi.fn(async () => ({ projects: [CURRENT_PROJECT] })), deviceId: "dev-1" });
  await vi.waitFor(() => expect(docsLabel()).toBeTruthy());
  expect(await readSourceSyncSupport("dev-1")).toBe(false);
  expect(document.querySelector("#sheet [data-sync-base]")).toBeNull();
});


it("keeps all legacy source edits and Sync without an ownership array on a pre-environment bridge", async () => {
  await greet(legacyGreeting);
  openProjectSettings("proj-1", { callRpc: vi.fn(async () => ({ projects: [PROJECT] })), deviceId: "dev-1" });
  await vi.waitFor(() => expect(docsLabel()?.readOnly).toBe(false));
  expect(document.querySelector('#sheet .ps-source[data-source-id="source-2"] [data-sync-base]')).toBeTruthy();
});
