// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { environmentDeviceSnapshot, snapshotEnvironments } from "./fixtures/environmentDeviceSnapshot.js";

const deviceId = "snapshot-device";
const address = kind => ({ deviceId, entityId: "", kind });
let cache, writer;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  const { createScopedDeviceSyncCache } = await import("../src/core/scopedDeviceSyncCache.js");
  writer = createScopedDeviceSyncCache({ deviceId, scope: { kind: "device" }, bridgeIdentity: deviceId });
});

it.each([true, false])("renders navigation and inbox from a device snapshot, with explicit added_sources: %s", async explicitAdded => {
  const records = structuredClone(environmentDeviceSnapshot.records);
  if (!explicitAdded) for (const record of records.filter(row => row.kind === "project")) delete record.row.added_sources;
  expect(records.find(row => row.kind === "project").row.sources).toBeUndefined();
  expect(await writer.publish({ records, capture: await writer.capture(), response: environmentDeviceSnapshot, snapshot: true })).toBe(true);
  await cache.writeCached(address("environments"), snapshotEnvironments);
  await cache.writeCached(address("environment-support"), { list: true, createProject: true });
  const { cachedFeedView } = await import("../src/core/cachedRows.js");
  const feed = await cachedFeedView(deviceId);
  expect(feed.projects).toHaveLength(2);
  for (const project of feed.projects) {
    expect(project).toMatchObject({ name: "build", environment_id: "env-1", sources: [{ id: "source-1" }] });
    expect(project.diverged).toBeUndefined();
    expect(project.environment_name).toBeUndefined();
  }
  const { readEnvironmentNavigation } = await import("../src/core/environmentNavigation.js");
  const context = await readEnvironmentNavigation([{ id: deviceId, name: "Snapshot device" }]);
  const { navigationCatalog } = await import("../src/core/navigationModel.js");
  const { globalNavigationPicker } = await import("../src/core/navigationPickerModel.js");
  const { globalPickerEntries } = await import("../src/core/navigationPickerRender.js");
  const picker = globalNavigationPicker(navigationCatalog(feed), context);
  expect(picker.groups.map(group => group.key)).toEqual([`${deviceId}/env-1`]);
  expect(picker.groups[0].destinations.map(row => row.key)).toEqual([`${deviceId}/proj-2`, `${deviceId}/proj-1`]);
  expect(globalPickerEntries(picker).map(row => row.html).join("")).not.toContain("folders");
  const { toolbarIdentity, workspaceMenuModel } = await import("../src/core/toolbarModel.js");
  const { projectEnvironmentIdentity } = await import("../src/core/environmentIdentity.js");
  const { toolbarHtml } = await import("../src/core/toolbarRender.js");
  const route = { name: "workspace", deviceId, projectId: "proj-1", workspaceId: "workspace-1", tab: "changes", sourceId: "source-1" };
  const identity = toolbarIdentity(route, feed);
  const environmentIdentity = projectEnvironmentIdentity(feed.projects[0], feed.projects, context);
  document.body.innerHTML = toolbarHtml({ ...identity, route, environmentIdentity });
  expect(document.querySelector("[data-project-return]").textContent).toContain("Shared environment › build");
  expect(document.querySelector('[data-select="workspace"]').textContent).toContain("Journal workspace");
  expect(workspaceMenuModel({ workspaces: feed.workspaces, projectKey: `${deviceId}/proj-1`, environmentsSupported: true }).map(row => row.name)).toEqual(["Journal workspace"]);
  const { workspaceProjectBlocks, projectHeadHtml } = await import("../src/core/inboxProjects.js");
  const blocks = workspaceProjectBlocks([], feed.projects, context.devices, null, Date.now(), new Set(), null, context).blocks;
  expect(blocks.map(block => block.environmentIdentity.environmentName)).toEqual(["Shared environment", "Shared environment"]);
  expect(projectHeadHtml(blocks[0])).toContain("Shared environment › build");
});
