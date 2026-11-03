// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

const owner = vi.hoisted(() => ({ session: "before", checks: 0 }));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => { owner.checks += 1; return owner.session; },
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
  owner.checks = 0;
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="task-page"></div>';
});
afterEach(() => page?.dispose());

it("does not publish a mounted task page full read after its device session changes", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const writer = vi.spyOn(tracker, "writeTaskRecord");
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
  const checksBeforeReply = owner.checks;
  owner.session = "after";
  resolve({ task: { ...held, title: "Retired task", body: "Retired body" }, timeline: [] });
  await vi.waitFor(() => expect(writer.mock.calls.length > 0 || owner.checks > checksBeforeReply).toBe(true));
  if (writer.mock.results[0]) await writer.mock.results[0].value;
  expect(await tracker.readTaskRecord(deviceId, projectId, taskId)).toBeNull();
  expect(document.querySelector(".task-page-title")?.textContent).not.toBe("Retired task");
  expect(callRpc).toHaveBeenCalledTimes(1);
});

it("requests one mounted full revalidation per body uncertainty and does not refill it on read-marker paints or errors", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const local = await import("../src/core/localCache.js");
  const { mergeTaskContentRecordsTogether, retireTaskContent } = await import("../src/core/taskContentAuthority.js");
  const notice = vi.spyOn(await import("../src/core/notify.js"), "notifyError");
  const writer = vi.spyOn(tracker, "writeTaskRecord");
  const address = tracker.taskAddress(deviceId, projectId, taskId);
  await local.writeCached(address, tracker.taskRecord(held, []));
  const requests = [];
  const callRpc = vi.fn(() => new Promise((resolve, reject) => { requests.push({ resolve, reject }); }));
  const adopted = vi.fn();
  const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
  page = mountTaskPage(document.querySelector("#task-page"), {
    deviceId, projectId, projectKey: `${deviceId}|${projectId}`, taskId, callRpc, onTaskRead: adopted,
    catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }), navigate: vi.fn(),
  });
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(1));
  requests[0].resolve({ task: held, timeline: [] });
  await vi.waitFor(() => expect(writer).toHaveBeenCalledTimes(1));
  await writer.mock.results[0].value;
  await mergeTaskContentRecordsTogether({ deviceId, projectId }, [address], ([record]) => ({
    values: [{ ...record.value, task: retireTaskContent(record.value.task, "body") }], retire: true,
  }));
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(2));
  for (let i = 0; i < 3; i += 1) {
    const before = adopted.mock.calls.length;
    await local.mergeCachedAtomically(address, (record) => ({ ...record, task: { ...record.task, read_through: `tc-${i}` } }));
    await vi.waitFor(() => expect(adopted.mock.calls.length).toBeGreaterThan(before));
    expect(callRpc).toHaveBeenCalledTimes(2);
  }
  requests[1].reject(new Error("Refused full revalidation"));
  await vi.waitFor(() => expect(notice).toHaveBeenCalledTimes(1));
  const before = adopted.mock.calls.length;
  await local.mergeCachedAtomically(address, (record) => ({ ...record, task: { ...record.task, read_through: "tc-last" } }));
  await vi.waitFor(() => expect(adopted.mock.calls.length).toBeGreaterThan(before));
  expect(callRpc).toHaveBeenCalledTimes(2);
  expect((await tracker.readTaskRecord(deviceId, projectId, taskId)).task.body).toBe(held.body);
});

it("coalesces a header-only summary barrier behind the mounted full read and suppresses identical headers after refusal or disposal", async () => {
  const local = await import("../src/core/localCache.js");
  const content = await import("../src/core/taskContentAuthority.js");
  const { summaryAddress } = await import("../src/core/taskSummaryCache.js");
  const scope = { deviceId, projectId }, address = summaryAddress(scope, taskId);
  const publishHeader = async (extra = {}) => {
    const contentRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    const candidate = content.observeTaskContent({ ...held, title: "Summary header", updated_at: "2026-10-29T00:00:00.000900Z", ...extra },
      { contentRead, coverage: "summary" });
    await content.mergeTaskContentRecordsTogether(scope, [address], ([prior], context) => ({
      values: [{ task: content.reconcileTaskContent(prior?.value?.task, candidate, { context }).task }], retire: false,
    }));
  };
  const notice = vi.spyOn(await import("../src/core/notify.js"), "notifyError");
  const requests = [];
  const callRpc = vi.fn(() => new Promise((resolve, reject) => { requests.push({ resolve, reject }); }));
  const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
  page = mountTaskPage(document.querySelector("#task-page"), {
    deviceId, projectId, projectKey: `${deviceId}|${projectId}`, taskId, callRpc,
    catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }), navigate: vi.fn(),
  });
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(1));
  await publishHeader();
  expect(callRpc).toHaveBeenCalledTimes(1);
  requests[0].resolve({ task: held, timeline: [] });
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(2));
  requests[1].reject(new Error("Refused full revalidation"));
  await vi.waitFor(() => expect(notice).toHaveBeenCalledTimes(1));
  const read = vi.spyOn(local, "readCachedMany");
  await publishHeader({ read_through: "tc-next" });
  const readsHeader = ([targets]) => targets.some((target) => target.kind === address.kind && target.sub === taskId);
  await vi.waitFor(() => expect(read.mock.calls.some(readsHeader)).toBe(true));
  const readIndex = read.mock.calls.findIndex(readsHeader);
  expect(read.mock.calls[readIndex][0].map((target) => target.kind)).toEqual(["tracker-task", address.kind]);
  await read.mock.results[readIndex].value;
  expect(callRpc).toHaveBeenCalledTimes(2);
  page.dispose();
  read.mockClear();
  await publishHeader({ title: "Changed after dispose" });
  expect(read.mock.calls.filter(readsHeader)).toHaveLength(0);
  expect(callRpc).toHaveBeenCalledTimes(2);
});
