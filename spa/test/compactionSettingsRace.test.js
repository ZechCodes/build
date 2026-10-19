// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { refreshCompactionCaches } from "../src/core/compactionCache.js";
import { deviceProjectsAddress, projectSettingsAddress } from "../src/core/settingsRecords.js";
import { deviceProjectsPanelHtml, mountDeviceProjects } from "../src/views/deviceProjects.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
afterEach(() => { document.body.innerHTML = ""; });

it("a device project list read and its detail copies cannot overtake an authoritative compaction refresh", async () => {
  await wipeCache();
  const project = (tokens) => ({ project_id: "project", name: "Project", path: "/project", compaction_thresholds: { codex: tokens } });
  const listAddress = deviceProjectsAddress("device");
  const detailAddress = projectSettingsAddress("device", "project");
  await writeCached(listAddress, [project(50)]);
  await writeCached(detailAddress, project(50));
  document.body.innerHTML = deviceProjectsPanelHtml();
  let finishOld;
  let finishFresh;
  const old = mountDeviceProjects(document.body, { deviceId: "device", deviceName: "Device",
    callRpc: () => new Promise((resolve) => { finishOld = resolve; }) });
  await vi.waitFor(() => expect(finishOld).toBeTypeOf("function"));
  const fresh = refreshCompactionCaches({ deviceId: "device", active: () => true,
    capabilities: { compaction: { harnessThresholds: true } },
    call: () => new Promise((resolve) => { finishFresh = resolve; }) }, { scope: "project", owner_id: "project" });
  await vi.waitFor(() => expect(finishFresh).toBeTypeOf("function"));
  finishOld({ projects: [project(100)] });
  await old;
  finishFresh({ projects: [project(200)] });
  await fresh;
  expect((await readCached(listAddress)).value[0].compaction_thresholds.codex).toBe(200);
  expect((await readCached(detailAddress)).value.compaction_thresholds.codex).toBe(200);
});
