/** @vitest-environment jsdom */
import { afterEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import { greetBridge, resetChangeEvents, bridgeCapabilities } from "../src/core/changeEvents.js";
import { adoptBridgeSelection, adoptDeviceSession, greetingInFlight, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { readCached } from "../src/core/localCache.js";
import { readTaskWaitingOnEverSupported, taskWaitingOnSupportAddress } from "../src/core/taskWaitingOnSupport.js";
import { openAssigneePicker } from "../src/core/trackerAssigneePicker.js";
import { assigneeOptions } from "../src/core/trackerAssignee.js";
import { fakeSession } from "./deviceSessionFixture.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
let picker;
afterEach(async () => {
  await picker?.close();
  picker = null;
  vi.restoreAllMocks();
  resetDeviceContexts();
  resetChangeEvents();
  document.body.replaceChildren();
});

const hello = (capable) => ({ api_version: "3.21.0", push_events: false,
  capabilities: capable ? ["tasks.waitingOn"] : [] });

function greet(context, answer) {
  const authority = greetingInFlight(context);
  return greetBridge(async () => answer, {
    deviceId: context.deviceId, isCurrent: authority.current,
    install: (selection) => {
      const adapter = selection.create(context.call);
      adoptBridgeSelection(context, selection, adapter, authority);
      return adapter;
    },
  });
}

async function submit(context, session) {
  picker = openAssigneePicker({ deviceId: context.deviceId, projectId: "project",
    task: { id: "ready-task", project_id: "project", title: "Ready task" },
    options: assigneeOptions([]), callRpc: context.rpc });
  const select = document.querySelector("[data-assignee-select]");
  select.value = "new_workspace";
  select.dispatchEvent(new Event("change"));
  const note = document.querySelector("#task-assign-note");
  note.value = "Keep this assessment";
  note.dispatchEvent(new Event("input"));
  document.querySelector("[data-assign-go]").click();
  await vi.waitFor(() => expect(session.call.mock.calls.length
    + Boolean(document.querySelector(".create-error")?.textContent)).toBeGreaterThan(0));
  expect(session.call).not.toHaveBeenCalled();
  expect(document.querySelector(".create-error").textContent).toMatch(/update.*bridge|bridge.*update/i);
  expect(note.value).toBe("Keep this assessment");
  expect(select.value).toBe("new_workspace");
}

it("refuses the real picker after a late capable hello is superseded by an incapable one", async () => {
  const session = fakeSession("history-late-hello");
  const context = adoptDeviceSession(session);
  let deliver;
  const late = greet(context, new Promise((resolve) => { deliver = resolve; }));
  await greet(context, hello(false));
  await vi.waitFor(async () => expect((await readCached(taskWaitingOnSupportAddress(context.deviceId)))?.value.waitingOn).toBe(false));
  deliver(hello(true));
  await late;
  expect(context.adapter.capabilities.tasks.waitingOn).toBe(false);
  expect(bridgeCapabilities(context.deviceId).tasks.waitingOn).toBe(false);
  await submit(context, session);
  expect(await readTaskWaitingOnEverSupported(context.deviceId)).toBe(true);
  await vi.waitFor(async () => expect((await readCached(taskWaitingOnSupportAddress(context.deviceId)))?.value)
    .toEqual({ waitingOn: false, everWaitingOn: true }));
});

it("retains capable evidence when IndexedDB refuses the first history put", async () => {
  const session = fakeSession("history-refused-put");
  const context = adoptDeviceSession(session);
  const put = IDBObjectStore.prototype.put;
  const refused = vi.fn();
  vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (value, key) {
    if (String(key).includes("history-refused-put|") && String(key).includes("task-waiting-on-support") && !refused.mock.calls.length) {
      refused();
      throw new DOMException("Injected history refusal", "QuotaExceededError");
    }
    return put.call(this, value, key);
  });
  await greet(context, hello(true));
  await vi.waitFor(() => expect(refused).toHaveBeenCalledOnce());
  vi.restoreAllMocks();
  expect(await readCached(taskWaitingOnSupportAddress(context.deviceId))).toBeUndefined();
  await greet(context, hello(false));
  await vi.waitFor(async () => expect((await readCached(taskWaitingOnSupportAddress(context.deviceId)))?.value.waitingOn).toBe(false));
  await submit(context, session);
  expect(document.querySelector(".create-error").textContent).toMatch(/retry|refresh/i);
  expect(await readTaskWaitingOnEverSupported(context.deviceId)).toBe(true);
});

it.each([true, false])("admits a current capable/confirmed legacy greeting: capable=%s", async (capable) => {
  const session = fakeSession(`history-control-${capable}`);
  const context = adoptDeviceSession(session);
  await greet(context, hello(capable));
  await vi.waitFor(async () => expect((await readCached(taskWaitingOnSupportAddress(context.deviceId)))?.value.waitingOn).toBe(capable));
  picker = openAssigneePicker({ deviceId: context.deviceId, projectId: "project",
    task: { id: "control", project_id: "project", title: "Control" },
    options: assigneeOptions([]), callRpc: context.rpc });
  const select = document.querySelector("[data-assignee-select]");
  select.value = "new_workspace";
  select.dispatchEvent(new Event("change"));
  document.querySelector("[data-assign-go]").click();
  await vi.waitFor(() => expect(session.call).toHaveBeenCalledOnce());
  expect(session.call.mock.calls[0][0]).toBe("tasks.assign");
});

it("discards evidence from a greeting whose original account has retired", async () => {
  const session = fakeSession("history-retired-account");
  const context = adoptDeviceSession(session);
  let deliver;
  const pending = greetBridge(() => new Promise((resolve) => { deliver = resolve; }), {
    deviceId: context.deviceId, isCurrent: () => false, isHistoryCurrent: () => context.active(),
  });
  resetDeviceContexts();
  deliver(hello(true));
  await pending;
  expect(await readTaskWaitingOnEverSupported(context.deviceId)).toBe(false);
});

it("fails closed through the real picker when the history read is refused", async () => {
  const session = fakeSession("history-refused-read");
  const context = adoptDeviceSession(session);
  await greet(context, hello(false));
  await vi.waitFor(async () => expect((await readCached(taskWaitingOnSupportAddress(context.deviceId)))?.value.waitingOn).toBe(false));
  const get = IDBObjectStore.prototype.get;
  vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function (key) {
    if (String(key).includes("history-refused-read|") && String(key).includes("task-waiting-on-support")) {
      throw new DOMException("Injected history read refusal", "SecurityError");
    }
    return get.call(this, key);
  });
  await submit(context, session);
  expect(document.querySelector(".create-error").textContent).toMatch(/retry|refresh/i);
});
