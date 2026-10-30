// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

const owner = vi.hoisted(() => ({ session: "before" }));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => owner.session,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/changeEvents.js", () => ({
  onBridgeGreeted: () => () => {},
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["tasks"] }, tasks: {} }),
  watchChanges: () => ({ dispose: () => {} }),
}));

const deviceId = "full-content-page", projectId = "proj-1", taskId = "task-42";
const held = task({ id: taskId, project_id: projectId, number: 42, title: "Held task", body: "Held body" });
let page;
beforeEach(() => {
  vi.resetModules();
  owner.session = "before";
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="task-page"></div>';
});
afterEach(() => page?.dispose());

it("does not publish a mounted task page full read after its device session changes", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  await tracker.writeTasksRecord(deviceId, projectId, tracker.tasksRecord([held], columns()));
  let resolve;
  const callRpc = vi.fn(() => new Promise((yes) => { resolve = yes; }));
  const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
  page = mountTaskPage(document.querySelector("#task-page"), {
    deviceId, projectId, projectKey: `${deviceId}|${projectId}`, taskId, callRpc,
    catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }), navigate: vi.fn(),
  });
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("tasks.get", { task_id: taskId }));
  owner.session = "after";
  resolve({ task: { ...held, title: "Retired task", body: "Retired body" }, timeline: [] });
  await new Promise((done) => setTimeout(done, 40));
  expect(await tracker.readTaskRecord(deviceId, projectId, taskId)).toBeNull();
  expect(document.querySelector(".task-page-title")?.textContent).not.toBe("Retired task");
  expect(callRpc).toHaveBeenCalledTimes(1);
});
