/** @vitest-environment jsdom */
// #228, with nothing mocked between the greeting and the sheet: the bridge's
// greeting (fixtures/api/v1/session.hello.json, which bridge/tests pins to the
// real reply) names `project.update_source`; the real greeting path writes it
// to the real cache; Project settings reads it there and offers each source's
// label, folder, base branch and remote for editing. A greeting without the
// name leaves a sheet whose one edit is the first source's remote.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { readSourceEditSupport } = await import("../src/core/sourceEditSupport.js");
const { openProjectSettings } = await import("../src/sheets/projectSettings.js");

const greeting = JSON.parse(readFileSync(resolve(process.cwd(), "../fixtures/api/v1/session.hello.json"), "utf8")).result;
const olderGreeting = { ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "project.update_source") };

const PROJECT = {
  project_id: "proj-1",
  name: "build",
  sources: [
    { id: "source-1", name: "build", mount: "build", path: "/b", is_git: true, base_branch: "main", remote: null },
    { id: "source-2", name: "docs", mount: "docs", path: "/d", is_git: true, base_branch: "main", remote: null },
  ],
};

beforeEach(() => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
});

afterEach(() => resetChangeEvents());

const greet = (hello) => greetBridge(async (method) => (method === "session.hello" ? hello : {}), { deviceId: "dev-1", strict: true });
const docsLabel = () => document.querySelector('#sheet .ps-source[data-source-id="source-2"] input[data-field="name"]');

it("offers every source's edits once the bridge's real greeting names project.update_source", async () => {
  expect(greeting.capabilities).toContain("project.update_source");
  await greet(greeting);
  await vi.waitFor(async () => expect(await readSourceEditSupport("dev-1")).toBe(true));
  openProjectSettings("proj-1", { callRpc: vi.fn(async () => ({ projects: [PROJECT] })), deviceId: "dev-1" });
  await vi.waitFor(() => expect(docsLabel()?.readOnly).toBe(false));
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
  openProjectSettings("proj-1", { callRpc: vi.fn(async () => ({ projects: [PROJECT] })), deviceId: "dev-1" });
  await vi.waitFor(() => expect(document.querySelector('#sheet .ps-source[data-source-id="source-2"] [data-sync-base]')).toBeTruthy());
});

it("offers no sync controls to a bridge whose greeting does not name sources.syncBase", async () => {
  const { readSourceSyncSupport } = await import("../src/core/sourceEditSupport.js");
  await greet({ ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "sources.syncBase") });
  openProjectSettings("proj-1", { callRpc: vi.fn(async () => ({ projects: [PROJECT] })), deviceId: "dev-1" });
  await vi.waitFor(() => expect(docsLabel()).toBeTruthy());
  expect(await readSourceSyncSupport("dev-1")).toBe(false);
  expect(document.querySelector("#sheet [data-sync-base]")).toBeNull();
});
