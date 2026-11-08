// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const runtime = vi.hoisted(() => ({ list: vi.fn(), call: vi.fn() }));
vi.mock("../src/core/writeOperationRuntime.js", () => ({
  deviceWriteOperations: () => ({ list: runtime.list }),
}));
vi.mock("../src/core/inboxDevices.js", () => ({ deviceCall: () => runtime.call }));

import { App } from "../src/appState.js";
import { readCreateWorkspaceTargets } from "../src/core/createWorkspaceTargets.js";
import { createWorkspaceWizardModel } from "../src/core/createWorkspaceWizardModel.js";
import { admitEnvironmentListWrite, environmentsAddress, writeCachedEnvironments } from "../src/core/environmentCache.js";
import { rememberEnvironmentSupport } from "../src/core/environmentSupport.js";
import { wipeCache, writeCached } from "../src/core/localCache.js";

const listAddress = (deviceId, kind) => ({ deviceId, entityId: "", kind });
const supported = { environments: { list: true, createProject: true } };
async function publishEnvironments(deviceId, rows) {
  const admission = admitEnvironmentListWrite(deviceId);
  expect(await writeCachedEnvironments(deviceId, rows, admission)).toBe(true);
}

beforeEach(async () => {
  globalThis.indexedDB ||= new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  await wipeCache();
  vi.clearAllMocks();
  runtime.list.mockResolvedValue([]);
  App.devices = [{ id: "laptop", name: "Laptop", status: "online" }];
});

describe("workspace targets with the environment cache", () => {
  it("keeps project creation available when a persisted environment list is malformed", async () => {
    await rememberEnvironmentSupport("laptop", supported);
    await writeCached(environmentsAddress("laptop"), { environments: "invalid old value" });
    await writeCached(listAddress("laptop", "projects"), [{ project_id: "p1", name: "Payments" }]);

    const snapshot = await readCreateWorkspaceTargets();

    expect(snapshot.environments).toEqual([]);
    expect(snapshot.projects).toMatchObject([{ id: "p1", deviceId: "laptop", name: "Payments" }]);
    expect(snapshot.support).toEqual({ laptop: true });
    expect(runtime.call).not.toHaveBeenCalled();
  });

  it("shows only the newest admitted environment publication in the wizard", async () => {
    await rememberEnvironmentSupport("laptop", supported);
    const obsolete = await admitEnvironmentListWrite("laptop");
    const newest = await admitEnvironmentListWrite("laptop");
    expect(await writeCachedEnvironments("laptop", [{ environment_id: "env-1", name: "Current" }], newest)).toBe(true);
    expect(await writeCachedEnvironments("laptop", [{ environment_id: "env-1", name: "Obsolete" }], obsolete)).toBe(false);

    const model = createWorkspaceWizardModel(await readCreateWorkspaceTargets());

    expect(model.sections.map((section) => section.kind)).toEqual(["environments"]);
    expect(model.sections[0].targets).toMatchObject([{ key: "laptop/env-1", name: "Current" }]);
    expect(JSON.stringify(model)).not.toContain("Obsolete");
  });

  it("keeps same-id targets and count wording attached to each device's cached rows", async () => {
    App.devices.push({ id: "desktop", name: "Desktop", status: "offline" });
    for (const device of App.devices) {
      await rememberEnvironmentSupport(device.id, supported);
      await publishEnvironments(device.id, [{ environment_id: "env-1", name: "Platform", sources: [{ path: `/${device.id}/code` }] }]);
      await writeCached(listAddress(device.id, "projects"), [{ project_id: "p1", name: "Payments", environment_id: "env-1", open_tasks: device.id === "laptop" ? 2 : 0, total_tasks: 9 }]);
    }
    const snapshot = await readCreateWorkspaceTargets();
    const model = createWorkspaceWizardModel(snapshot);

    expect(snapshot.support).toEqual({ laptop: true, desktop: true });
    const projects = model.sections.find((section) => section.kind === "projects").targets;
    expect(projects).toEqual(expect.arrayContaining([
      expect.objectContaining({ key: "laptop/p1", deviceLabel: "Laptop", offline: false, taskText: "2 open tasks", sources: [{ path: "/laptop/code" }] }),
      expect.objectContaining({ key: "desktop/p1", deviceLabel: "Desktop", offline: true, taskText: "9 tasks", sources: [{ path: "/desktop/code" }] }),
    ]));
    expect(model.sections.find((section) => section.kind === "environments").targets.map((target) => target.key).sort()).toEqual(["desktop/env-1", "laptop/env-1"]);
    expect(runtime.call).not.toHaveBeenCalled();
  });

  it("retains project rows when cached support is downgraded and hides environment creation targets", async () => {
    await rememberEnvironmentSupport("laptop", supported);
    await publishEnvironments("laptop", [{ environment_id: "env-1", name: "Platform" }]);
    await writeCached(listAddress("laptop", "projects"), [{ project_id: "p1", name: "Existing board" }]);
    await rememberEnvironmentSupport("laptop", { environments: { list: true, createProject: false } });

    const model = createWorkspaceWizardModel(await readCreateWorkspaceTargets());

    expect(model.sections.map((section) => section.kind)).toEqual(["projects"]);
    expect(model.sections[0].targets[0].name).toBe("Existing board");
    expect(model.legacyNote).toBe("This device needs an update to create environments here");
  });
});
it("reads new-environment and added-folder rendering support from persisted cache flags",async()=>{
 await rememberEnvironmentSupport('laptop',{environments:{list:true,createProject:true,create:true,addedSources:true}});
 const snapshot=await readCreateWorkspaceTargets();
 expect(snapshot.features.laptop).toMatchObject({create:true,addedSources:true});
 expect(createWorkspaceWizardModel(snapshot).newEnvironmentDevices).toEqual(App.devices);
});
