/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { greetBridge, resetChangeEvents } from "../src/core/changeEvents.js";
import { readCached, writeCached } from "../src/core/localCache.js";
import { taskWaitingOnSupportAddress } from "../src/core/taskWaitingOnSupport.js";
import {
  taskAddress, tasksAddress, tasksQueryAddress,
  writeTaskRecord, writeTasksRecord, writeTasksQueryRecord,
} from "../src/core/trackerCache.js";
import { openAssigneePicker } from "../src/core/trackerAssigneePicker.js";
import { assigneeOptions } from "../src/core/trackerAssignee.js";

const session = vi.hoisted(() => ({ context: null }));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => session.context,
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
}));

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
let picker;
beforeEach(() => { document.body.replaceChildren(); });
afterEach(async () => {
  await picker?.close();
  picker = null;
  session.context = null;
  resetChangeEvents();
});

async function greet(deviceId, waitingOn) {
  session.context = { deviceId, adapter: null };
  await greetBridge(async () => ({
    api_version: "3.21.0", push_events: false,
    capabilities: waitingOn ? ["tasks.waitingOn"] : [],
  }), {
    deviceId,
    install: (selection) => {
      const adapter = selection.create(vi.fn());
      session.context.adapter = adapter;
      return adapter;
    },
  });
  // Capability writes settle independently of hello; await their committed
  // value before another greeting replaces this session.
  await vi.waitFor(async () => {
    expect((await readCached(taskWaitingOnSupportAddress(deviceId)))?.value.waitingOn).toBe(waitingOn);
  });
}

const sourceAddress = (source, deviceId, taskId) => ({
  detail: taskAddress(deviceId, "project", taskId),
  catalogue: tasksAddress(deviceId, "project"),
  query: tasksQueryAddress(deviceId, "project", { label: "blocked" }),
})[source];

const legacyWrite = (source, deviceId, task) => ({
  detail: () => writeTaskRecord(deviceId, "project", task.id, { task, timeline: [] }),
  catalogue: () => writeTasksRecord(deviceId, "project", { tasks: [task] }),
  query: () => writeTasksQueryRecord(deviceId, "project", { label: "blocked" }, { tasks: [task] }),
})[source]();

const cases = ["detail", "catalogue", "query"].flatMap((source) =>
  ["absent", "first-ready"].map((knowledge) => ({ source, knowledge })));

describe("real assignment picker after a bridge downgrade", () => {
  it.each(cases)("refuses $source legacy erasure with $knowledge dependency knowledge", async ({ source, knowledge }) => {
    const deviceId = `downgrade-${source}-${knowledge}`;
    const ready = {
      id: `task-${source}-${knowledge}`, project_id: "project", number: 42,
      title: "Retained task", updated_at: "2026-10-13T01:00:00Z",
      dependencies: { blocked: false, waiting_on: [] },
    };
    const blocked = {
      ...ready, updated_at: "2026-10-13T01:01:00Z",
      dependencies: { blocked: true, waiting_on: [{ task_id: "prerequisite", resolved: false }] },
    };
    await greet(deviceId, true);
    // A released client left a capable replica before dependency-knowledge
    // records existed. Reading it has no observation side effect.
    await writeCached(sourceAddress(source, deviceId, ready.id),
      source === "detail" ? { task: blocked, timeline: [] } : { tasks: [blocked] });

    const call = vi.fn(async () => ({}));
    picker = openAssigneePicker({ deviceId, projectId: "project", task: ready,
      options: assigneeOptions([]), callRpc: call });
    const selection = document.querySelector("[data-assignee-select]");
    selection.value = "new_workspace";
    selection.dispatchEvent(new Event("change"));
    const note = document.querySelector("#task-assign-note");
    note.value = "Keep my prerequisite assessment";
    note.dispatchEvent(new Event("input"));
    if (knowledge === "first-ready") {
      // An older capable response first establishes ready knowledge while a
      // newer blocker still exists in another retained replica.
      await writeTasksQueryRecord(deviceId, "project", { label: "old-read" }, { tasks: [ready] });
    }
    await greet(deviceId, false);
    const legacy = { ...blocked };
    delete legacy.dependencies;
    await legacyWrite(source, deviceId, legacy);
    const erased = (await readCached(sourceAddress(source, deviceId, ready.id))).value;
    expect((erased.task || erased.tasks[0]).dependencies).toBeUndefined();

    document.querySelector("[data-assign-go]").click();
    await vi.waitFor(() => {
      expect(call.mock.calls.length + Boolean(document.querySelector(".create-error")?.textContent)).toBeGreaterThan(0);
    });
    expect(call).not.toHaveBeenCalled();
    expect(document.querySelector(".create-error").textContent).toMatch(/bridge was downgraded.*update/i);
    expect(document.querySelector("#task-assign-note").value).toBe("Keep my prerequisite assessment");
    expect(document.querySelector("[data-assignee-select]").value).toBe("new_workspace");
    expect(document.querySelector("[data-assign-go]").disabled).toBe(false);
  });
});
