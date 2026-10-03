/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { createHash, webcrypto } from "node:crypto";
import { columns, comment, task } from "./trackerWireFixture.js";

let watchers = [];
vi.mock("../src/core/changeEvents.js", () => ({
  onBridgeGreeted: () => () => {},
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["tasks"] }, tasks: {} }),
  watchChanges: (registration) => {
    watchers.push(registration);
    return { dispose: () => {} };
  },
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => true, reconnecting: () => true, moved: () => () => {} }),
}));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args) }));

const DEVICE = "dev-347";
const PROJECT = "proj-1";
const TASK = "task-347";
const BODY = "Release\r\n\r\n- [ ] **Verify**\r\n- [X] Publish\r\n\r\n```md\r\n- [ ] Example\r\n```";
const AFTER = BODY.replace("[ ] **Verify**", "[x] **Verify**");
const hash = (body) => createHash("sha256").update(body).digest("hex");
let host, page, cache, call, saved, reads, writes;
const box = (index = 0) => host.querySelector(`.task-page-body input[data-task-index="${index}"]`);
const cached = () => cache.readTaskRecord(DEVICE, PROJECT, TASK);
const push = () => watchers.forEach((one) => one.onChanges?.([{ tasks: { task_ids: [TASK] } }]));
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const wire = (body = BODY) => ({
  task: task({ id: TASK, number: 347, body }),
  timeline: [comment({ id: "tc-347", task_id: TASK, body: "- [x] Reviewed\n- [ ] Discuss" })],
});
const mount = async () => {
  const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
  page = mountTaskPage(host, {
    deviceId: DEVICE, projectId: PROJECT, projectKey: `${DEVICE}|${PROJECT}`, taskId: TASK,
    callRpc: call, catalog: () => ({ providers: [] }), feed: () => ({ items: [], workspaces: [] }),
  });
  await vi.waitFor(() => expect(box()).not.toBeNull());
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  vi.stubGlobal("crypto", webcrypto);
  watchers = [];
  notifyError.mockClear();
  reads = [];
  writes = [];
  document.body.innerHTML = '<div id="pane"></div>';
  host = document.querySelector("#pane");
  cache = await import("../src/core/trackerCache.js");
  saved = wire();
  await cache.writeTaskRecord(DEVICE, PROJECT, TASK, saved);
  await cache.writeTasksRecord(DEVICE, PROJECT, cache.tasksRecord([saved.task], columns()));
  const { rememberTaskChecklistSupport } = await import("../src/core/taskChecklistSupport.js");
  await rememberTaskChecklistSupport(DEVICE, { tasks: { bodyPrecondition: true } });
  call = vi.fn((method, params) => {
    const pending = deferred();
    if (method === "tasks.get") { reads.push(pending); return pending.promise; }
    if (method === "tasks.update") { writes.push({ ...pending, params }); return pending.promise; }
    return Promise.resolve({});
  });
});

afterEach(() => { page?.dispose(); vi.unstubAllGlobals(); });

it("offers cached body boxes before a greeting, and shows disabled boxes in comments", async () => {
  await mount();
  expect(box().disabled).toBe(false);
  expect(box(1).checked).toBe(true);
  const comments = [...host.querySelectorAll('.task-comment-body input[type="checkbox"]')];
  expect(comments.map((input) => [input.checked, input.disabled])).toEqual([[true, true], [false, true]]);
  comments[1].click();
  expect(writes).toHaveLength(0);
});

it("writes only the chosen marker to cache before saving, then survives a remount", async () => {
  await mount();
  const field = host.querySelector("#task-comment");
  field.value = "Unsent draft";
  field.dispatchEvent(new Event("input", { bubbles: true }));
  box().focus();
  box().click();
  await vi.waitFor(() => expect(writes).toHaveLength(1));
  expect(writes[0].params).toEqual({ task_id: TASK, body: AFTER, expected_body_hash: hash(BODY) });
  expect((await cached()).task.body).toBe(AFTER);
  expect(box().checked).toBe(true);
  expect(host.querySelector("#task-comment")).toBe(field);
  expect(field.value).toBe("Unsent draft");
  writes[0].resolve({ task: wire(AFTER).task });
  await vi.waitFor(() => expect(box().disabled).toBe(false));
  expect(document.activeElement).toBe(box());
  push();
  await vi.waitFor(() => expect(reads.length).toBeGreaterThan(0));
  reads[0].resolve(wire(AFTER));
  await vi.waitFor(() => expect(reads.length).toBeGreaterThan(1));
  reads[1].resolve(wire(AFTER));
  await vi.waitFor(() => expect(box().checked).toBe(true));
  page.dispose();
  host.innerHTML = "";
  await mount();
  expect(box().checked).toBe(true);
});

it("does not let a pre-save read undo the optimistic tick while saving", async () => {
  await mount();
  await vi.waitFor(() => expect(reads).toHaveLength(1));
  box().click();
  await vi.waitFor(() => expect(writes).toHaveLength(1));
  reads[0].resolve(saved);
  await vi.waitFor(() => expect(box().checked).toBe(true));
  // Wait for the stale response's asynchronous cache transaction too.
  await new Promise((resolve) => setTimeout(resolve, 30));
  expect((await cached()).task.body).toBe(AFTER);
  expect(box().checked).toBe(true);
  writes[0].resolve({ task: wire(AFTER).task });
  await vi.waitFor(() => expect(reads).toHaveLength(2));
  reads[1].resolve(wire(AFTER));
  await vi.waitFor(() => expect(box().disabled).toBe(false));
});

it("reverts a rejected tick through cache without replacing newer task fields", async () => {
  await mount();
  box().click();
  await vi.waitFor(() => expect(writes).toHaveLength(1));
  await cache.writeTaskRecord(DEVICE, PROJECT, TASK, {
    ...wire(AFTER), task: { ...wire(AFTER).task, title: "New title" },
  });
  writes[0].reject(new Error("Machine is away"));
  await vi.waitFor(() => expect(notifyError).toHaveBeenCalledWith("Could not save this checklist", "Machine is away"));
  await vi.waitFor(() => expect(box().checked).toBe(false));
  expect((await cached()).task.title).toBe("New title");
  expect((await cached()).task.body).toBe(BODY);
  expect(box().disabled).toBe(false);
});

it("does not roll back a body another cache writer has replaced", async () => {
  await mount();
  box().click();
  await vi.waitFor(() => expect(writes).toHaveLength(1));
  const newer = "- [x] Revised checklist";
  await cache.writeTaskRecord(DEVICE, PROJECT, TASK, wire(newer));
  writes[0].reject(new Error("Save refused"));
  await vi.waitFor(() => expect(notifyError).toHaveBeenCalled());
  expect((await cached()).task.body).toBe(newer);
});

it("serializes saves and builds the next tick from the updated cached body", async () => {
  await mount();
  box().click();
  await vi.waitFor(() => expect(writes).toHaveLength(1));
  expect(box(1).disabled).toBe(true);
  box(1).click();
  expect(writes).toHaveLength(1);
  writes[0].resolve({ task: wire(AFTER).task });
  await vi.waitFor(() => expect(box(1).disabled).toBe(false));
  box(1).click();
  await vi.waitFor(() => expect(writes).toHaveLength(2));
  expect(writes[1].params.body).toBe(AFTER.replace("[X] Publish", "[ ] Publish"));
  expect(writes[1].params.expected_body_hash).toBe(hash(AFTER));
});

it("rolls back a tick refused by a newer server body, then refreshes that body", async () => {
  let serverBody = BODY + "\r\n\r\nAn agent added this instruction.";
  const pendingRead = deferred();
  let serverReads = 0;
  call.mockImplementation((method, params) => {
    if (method === "tasks.get") {
      serverReads += 1;
      return serverReads === 1 ? pendingRead.promise : Promise.resolve(wire(serverBody));
    }
    if (method !== "tasks.update") return Promise.resolve({});
    writes.push({ params });
    if (params.expected_body_hash !== hash(serverBody)) {
      return Promise.reject(Object.assign(new Error("stale_body: The task body changed. Refresh the task before editing it."), { code: "stale_body" }));
    }
    serverBody = params.body;
    return Promise.resolve({ task: wire(serverBody).task });
  });
  await mount();
  await vi.waitFor(() => expect(serverReads).toBe(1));
  box().click();
  await vi.waitFor(() => expect(notifyError).toHaveBeenCalledExactlyOnceWith("This task changed elsewhere; reloaded it."));
  expect(writes).toHaveLength(1);
  expect(writes[0].params.expected_body_hash).toBe(hash(BODY));
  expect(serverBody).toContain("An agent added this instruction.");
  expect((await cached()).task.body).toBe(BODY);
  expect(box().checked).toBe(false);
  pendingRead.resolve(wire());
  await vi.waitFor(async () => expect((await cached()).task.body).toBe(serverBody));
  expect(serverReads).toBe(2);
  expect(writes).toHaveLength(1);
  expect(host.querySelector(".task-page-body").textContent).toContain("An agent added this instruction.");
});

it("keeps cached checklists disabled until safe body writes are advertised", async () => {
  const { rememberTaskChecklistSupport } = await import("../src/core/taskChecklistSupport.js");
  await rememberTaskChecklistSupport(DEVICE, { tasks: {} });
  await mount();
  expect(box().disabled).toBe(true);
  box().click();
  expect(writes).toHaveLength(0);
  await rememberTaskChecklistSupport(DEVICE, { tasks: { bodyPrecondition: true } });
  await vi.waitFor(() => expect(box().disabled).toBe(false));
});

it("keeps checkbox focus after an immediate save and its later cache repaint", async () => {
  call.mockImplementation((method, params) => method === "tasks.update"
    ? Promise.resolve({ task: wire(params.body).task }) : new Promise(() => {}));
  await mount();
  box().focus();
  box().click();
  await vi.waitFor(() => expect(box().checked && !box().disabled).toBe(true));
  await new Promise((resolve) => setTimeout(resolve, 100));
  expect(document.activeElement).toBe(box());
});

it("keeps a pending save protected when the task page is reopened", async () => {
  await mount();
  box().click();
  await vi.waitFor(() => expect(writes).toHaveLength(1));
  page.dispose();
  host.innerHTML = "";
  await mount();
  await new Promise((resolve) => setTimeout(resolve, 50));
  if (reads[1]) reads[1].resolve(wire());
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect((await cached()).task.body).toBe(AFTER);
  expect(box().disabled).toBe(true);
  writes[0].resolve({ task: wire(AFTER).task });
  await vi.waitFor(() => expect(box().disabled).toBe(false));
});

it("restores checkbox availability if the browser refuses its write lock", async () => {
  await mount();
  const previous = Object.getOwnPropertyDescriptor(navigator, "locks");
  Object.defineProperty(navigator, "locks", { configurable: true, value: {
    request: () => Promise.reject(new Error("Storage access refused")),
  } });
  try {
    box().click();
    await vi.waitFor(() => expect(notifyError).toHaveBeenCalledWith("Could not save this checklist", "Storage access refused"));
    await vi.waitFor(() => expect(box().checked).toBe(false));
    expect(box().disabled).toBe(false);
    expect(writes).toHaveLength(0);
    expect((await cached()).task.body).toBe(BODY);
  } finally {
    if (previous) Object.defineProperty(navigator, "locks", previous);
    else delete navigator.locks;
  }
});
