/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { assigneePickerBodyHtml, openAssigneePicker } from "../src/core/trackerAssigneePicker.js";
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
