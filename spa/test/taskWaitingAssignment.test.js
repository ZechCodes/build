/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { assigneePickerBodyHtml, openAssigneePicker } from "../src/core/trackerAssigneePicker.js";
import { writeTaskRecord } from "../src/core/trackerCache.js";
import { assigneeOptions } from "../src/core/trackerAssignee.js";
import { emptyAssigneeDraft } from "../src/core/trackerAssigneeControl.js";

const greeting = vi.hoisted(() => ({ supported: true }));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => ({ deviceId: "device", adapter: { capabilities: { tasks: { waitingOn: greeting.supported } } } }),
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ tasks: { waitingOn: greeting.supported } }),
}));

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
let handle;
beforeEach(() => { greeting.supported = true; document.body.replaceChildren(); });
afterEach(async () => { await handle?.close(); handle = null; });
const settle = async () => {
  for (let i = 0; i < 20; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

const picker = (task, optionId = "new_workspace", busy = false) => assigneePickerBodyHtml({
  task: { id: "task-one", number: 1, title: "Build it", ...task },
  options: assigneeOptions([]), draft: emptyAssigneeDraft(optionId), catalog: null,
  note: "Keep my note", error: "", busy,
});

describe("waiting assignment form", () => {
  it("retains the note and target when an older current greeting refuses a waiting form", async () => {
    const call = vi.fn();
    handle = openAssigneePicker({
      deviceId: "device", projectId: "project", task: { id: "task-one", project_id: "project", number: 1,
        title: "Build it", dependencies: { blocked: true } }, options: assigneeOptions([]),
      current: "new_workspace", callRpc: call,
    });
    const note = document.querySelector("#task-assign-note");
    note.value = "Check the closed prerequisite";
    note.dispatchEvent(new Event("input"));
    greeting.supported = false;
    document.querySelector("[data-assign-go]").click();
    await settle();
    expect(call).not.toHaveBeenCalled();
    expect(document.querySelector("#task-assign-note").value).toBe("Check the closed prerequisite");
    expect(document.querySelector("[data-assignee-select]").value).toBe("new_workspace");
    expect(document.querySelector(".create-error").textContent).toMatch(/update|bridge/i);
  });

  it("retains a ready form when cached facts reblock it before submission to an older greeting", async () => {
    const task = { id: "task-reblocked", project_id: "project", number: 2,
      title: "Build it", dependencies: { blocked: false, waiting_on: [] } };
    await writeTaskRecord("device", "project", task.id, { task, timeline: [] });
    const call = vi.fn();
    handle = openAssigneePicker({ deviceId: "device", projectId: "project", task,
      options: assigneeOptions([]), current: "new_workspace", callRpc: call });
    const note = document.querySelector("#task-assign-note");
    note.value = "Assess the prerequisite";
    note.dispatchEvent(new Event("input"));
    await writeTaskRecord("device", "project", task.id, {
      task: { ...task, dependencies: { blocked: true, waiting_on: [{ task_id: "new-prerequisite", resolved: false }] } }, timeline: [],
    });
    greeting.supported = false;
    document.querySelector("[data-assign-go]").click();
    await settle();
    expect(call).not.toHaveBeenCalled();
    expect(document.querySelector("#task-assign-note").value).toBe("Assess the prerequisite");
    expect(document.querySelector("[data-assignee-select]").value).toBe("new_workspace");
    expect(document.querySelector(".create-error").textContent).toMatch(/update.*bridge/i);
  });

  it("accepts a saved creating assignment with a null dispatch", async () => {
    const onAssigned = vi.fn();
    const call = vi.fn(async () => ({ task: { id: "task-one", project_id: "project", number: 1,
      assignee: null, dependencies: { blocked: true },
      deferred_assignment: { id: "saved-one", state: "waiting", target: { kind: "new_workspace" } } }, dispatch: null }));
    handle = openAssigneePicker({ deviceId: "device", projectId: "project",
      task: { id: "task-one", project_id: "project", number: 1, title: "Build it", dependencies: { blocked: true } },
      options: assigneeOptions([]), current: "new_workspace", callRpc: call, onAssigned,
    });
    document.querySelector("[data-assign-go]").click();
    await settle();
    expect(call).toHaveBeenCalledExactlyOnceWith("tasks.assign", {
      task_id: "task-one", assignee: { kind: "new_workspace", provider: "claude_adk" },
    });
    expect(onAssigned).toHaveBeenCalledWith(expect.objectContaining({ dispatch: null,
      task: expect.objectContaining({ deferred_assignment: expect.objectContaining({ id: "saved-one" }) }) }));
  });
});

describe("waiting assignment copy", () => {
  it("explains Done or closed, delayed creation and closure assessment", () => {
    const html = picker({ dependencies: { blocked: true, waiting_on: [{ task_id: "task-two", resolved: false }] } });
    expect(html).toContain("Done or closed");
    expect(html).toContain("created when the task unblocks");
    expect(html).toContain("assesses prerequisites closed without Done");
    expect(html).toContain("Assign when ready");
    expect(html).not.toContain("Cuts a workspace in this project and starts an agent on it.");
  });

  it("does not claim provisioning while saving a blocked assignment", () => {
    const html = picker({ dependencies: { blocked: true } }, "new_workspace", true);
    expect(html).toContain("saving assignment…");
    expect(html).not.toContain("Cutting the checkout");
    expect(html).not.toContain("cutting the workspace…");
  });

  it("keeps a ready assignment immediate and explains retained closures", () => {
    const html = picker({ dependencies: { blocked: false, waiting_on: [{ task_id: "closed", resolved: true, resolved_by: "close" }] } });
    expect(html).toContain("Assign and start");
    expect(html).toContain("assesses prerequisites closed without Done");
  });

  it("does not promise a worker for a user assignment", () => {
    const html = picker({ dependencies: { blocked: true } }, "user");
    expect(html).toContain("Done or closed");
    expect(html).toContain("No agent starts for a user assignment");
  });
});

describe("assignment picker dependency knowledge migration", () => {
  it.each(["detail", "catalogue", "query", "page"])(
    "retains a pre-existing %s blocker when an older ready response first establishes knowledge",
    async (source) => {
      const local = await import("../src/core/localCache.js");
      const tracker = await import("../src/core/trackerCache.js");
      const facts = await import("../src/core/taskAssignmentFacts.js");
      const ready = { id: `migration-${source}`, project_id: "project", number: 42,
        title: "Retained task", updated_at: "2026-10-13T01:00:00Z", dependencies: { blocked: false, waiting_on: [] } };
      const blocked = { ...ready, updated_at: "2026-10-13T01:01:00Z",
        dependencies: { blocked: true, waiting_on: [{ task_id: "prerequisite", resolved: false }] } };
      const addresses = {
        detail: tracker.taskAddress("device", "project", ready.id),
        catalogue: tracker.tasksAddress("device", "project"),
        query: tracker.tasksQueryAddress("device", "project", { label: "retained" }),
        page: tracker.tasksPageAddress("device", "project", { cursor: "retained" }),
      };
      // Released-client cache schema: the capable replica predates knowledge.
      await local.writeCached(addresses[source], source === "detail" ? { task: blocked, timeline: [] } : { tasks: [blocked] });
      expect((await facts.readTaskAssignmentFacts("device", "project", ready.id, blocked)).dependencies.blocked).toBe(true);
      const call = vi.fn(async () => ({}));
      handle = openAssigneePicker({ deviceId: "device", projectId: "project", task: blocked,
        options: assigneeOptions([]), current: "new_workspace", callRpc: call });
      const note = document.querySelector("#task-assign-note");
      note.value = "Keep my draft";
      note.dispatchEvent(new Event("input"));
      // An older capable response establishes the first knowledge record.
      await tracker.writeTasksQueryRecord("device", "project", { label: "old-read" }, { tasks: [ready] });
      greeting.supported = false;
      document.querySelector("[data-assign-go]").click();
      await settle();
      expect(call).not.toHaveBeenCalled();
      expect(document.querySelector("#task-assign-note").value).toBe("Keep my draft");
      expect(document.querySelector("[data-assignee-select]").value).toBe("new_workspace");
      expect(document.querySelector(".create-error").textContent).toMatch(/update.*bridge/i);
    },
  );

  it("retains an opened-only newer blocker when an older ready record appears", async () => {
    const ready = { id: "migration-opened-only", project_id: "project", number: 43,
      title: "Opened blocker", updated_at: "2026-10-13T01:00:00Z", dependencies: { blocked: false, waiting_on: [] } };
    const blocked = { ...ready, updated_at: "2026-10-13T01:01:00Z",
      dependencies: { blocked: true, waiting_on: [{ task_id: "prerequisite", resolved: false }] } };
    const call = vi.fn(async () => ({}));
    handle = openAssigneePicker({ deviceId: "device", projectId: "project", task: blocked,
      options: assigneeOptions([]), current: "new_workspace", callRpc: call });
    const note = document.querySelector("#task-assign-note");
    note.value = "Keep the opened draft";
    note.dispatchEvent(new Event("input"));
    await writeTaskRecord("device", "project", ready.id, { task: ready, timeline: [] });
    greeting.supported = false;
    document.querySelector("[data-assign-go]").click();
    await settle();
    expect(call).not.toHaveBeenCalled();
    expect(document.querySelector("#task-assign-note").value).toBe("Keep the opened draft");
    expect(document.querySelector("[data-assignee-select]").value).toBe("new_workspace");
  });
});
