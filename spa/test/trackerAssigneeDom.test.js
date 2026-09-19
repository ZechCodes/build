/** @vitest-environment jsdom */
// The assignee picker and the new-issue form — the two surfaces that ask the
// same question with the same control.
//
// Assigning IS dispatching, so the control says what the chosen option is about
// to do before it is pressed, and the two creating kinds are the only ones that
// open the harness/model/effort selects.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { assigneeOptions, workspaceAgents } from "../src/core/trackerAssignee.js";
import { openAssigneePicker } from "../src/core/trackerAssigneePicker.js";
import { createIssueParams, labelsFromText, openCreateIssue } from "../src/core/trackerCreate.js";
import { issue } from "./trackerWireFixture.js";

const PROJECT_KEY = "dev-1|proj-1";

const feed = {
  workspaces: [{ id: "ws-1", workspace_id: "ws-1", name: "wire-facade", projectKey: PROJECT_KEY, entity_id: "run-1" }],
  items: [{ kind: "branch", projectKey: PROJECT_KEY, run_id: "run-1", agents: [{ id: "agent-1", ordinal: 1 }] }],
};

const CATALOG = {
  default_provider: "claude",
  providers: [
    { id: "claude", label: "Claude Code", models: [{ id: "opus", label: "Opus" }], efforts: ["low", "high"] },
  ],
};

const flush = async () => {
  for (let i = 0; i < 10; i++) await new Promise((done) => setTimeout(done, 0));
};

const options = () => assigneeOptions(workspaceAgents(feed, PROJECT_KEY));

let call, handle;

const choose = async (id) => {
  const select = document.querySelector("[data-assignee-select]");
  select.value = id;
  select.dispatchEvent(new Event("change"));
  await flush();
};

const press = (selector) => document.querySelector(selector).click();

beforeEach(() => {
  document.body.innerHTML = "";
  call = vi.fn(async () => ({ issue: issue({ id: "issue-1" }), dispatch: null }));
});

afterEach(async () => {
  await handle?.close?.();
  handle = null;
});

describe("the picker", () => {
  const open = (over = {}) => {
    handle = openAssigneePicker({
      issue: issue({ id: "issue-1", number: 12 }),
      options: options(),
      current: "none",
      catalog: CATALOG,
      callRpc: call,
      ...over,
    });
    return handle;
  };

  it("offers all five kinds, grouped by workspace", () => {
    open();
    expect([...document.querySelectorAll("[data-assignee-select] option")].map((one) => one.value)).toEqual([
      "none", "user", "project_agent", "agent:agent-1", "new_agent:ws-1", "new_workspace",
    ]);
    expect(document.querySelector("optgroup").getAttribute("label")).toBe("wire-facade");
  });

  // The reader should not discover afterwards that choosing a name cut a
  // workspace.
  it("says what the chosen option is about to do", async () => {
    open();
    expect(document.querySelector(".issue-assignee-hint").textContent).toBe("Nobody holds it. Nothing running is stopped.");
    await choose("new_workspace");
    expect(document.querySelector(".issue-assignee-hint").textContent)
      .toBe("Cuts a workspace in this project and starts an agent on it.");
  });

  it("says as much on the press itself", async () => {
    open();
    await choose("user");
    expect(document.querySelector("[data-assign-go]").textContent).toBe("Assign");
    await choose("new_workspace");
    expect(document.querySelector("[data-assign-go]").textContent).toBe("Assign and start");
  });

  // The harness/model/effort selects belong to the two kinds that create an
  // agent; an agent that already exists is locked to its own harness.
  it("opens the agent-choice controls on the two creating kinds only", async () => {
    open();
    await choose("agent:agent-1");
    expect(document.querySelector(".agent-choice")).toBeNull();
    await choose("new_agent:ws-1");
    expect(document.querySelector(".agent-choice")).not.toBeNull();
    expect(document.querySelector("#issue-assign-name")).toBeNull();
  });

  it("asks a new workspace for its name and isolation, and nothing else for it", async () => {
    open();
    await choose("new_workspace");
    expect(document.querySelector("#issue-assign-name")).not.toBeNull();
    expect(document.querySelector("#issue-assign-isolation")).not.toBeNull();
  });

  it("assigns through issues.assign with the tagged shape", async () => {
    open();
    await choose("agent:agent-1");
    press("[data-assign-go]");
    await flush();
    expect(call).toHaveBeenCalledWith("issues.assign", {
      issue_id: "issue-1", assignee: { kind: "agent", agent_id: "agent-1" },
    });
  });

  it("unassigns with a null assignee", async () => {
    open();
    press("[data-assign-go]");
    await flush();
    expect(call.mock.calls[0][1].assignee).toBeNull();
  });

  // A hand-off note belongs in the conversation it was said in, not on the
  // issue.
  it("carries a note when one was written, and nothing when it was not", async () => {
    open();
    await choose("project_agent");
    const note = document.querySelector("#issue-assign-note");
    note.value = "  look at the drag handler  ";
    note.dispatchEvent(new Event("input"));
    press("[data-assign-go]");
    await flush();
    expect(call.mock.calls[0][1].note).toBe("look at the drag handler");
  });

  // Absent is absent: agent.add reads a key's presence to tell "run it on this"
  // from "run it on whatever the workspace runs on".
  it("sends no harness, model or effort when none was chosen", async () => {
    open();
    await choose("new_workspace");
    press("[data-assign-go]");
    await flush();
    expect(call.mock.calls[0][1].assignee).toEqual({ kind: "new_workspace", provider: "claude" });
  });

  it("keeps the dialog up and says why when the assign is refused", async () => {
    call = vi.fn(async () => {
      throw new Error("agent agent-1 is not in project proj-1");
    });
    open({ callRpc: call });
    await choose("agent:agent-1");
    press("[data-assign-go]");
    await flush();
    expect(document.querySelector(".create-error").textContent).toBe("agent agent-1 is not in project proj-1");
    expect(document.querySelector("[data-assign-go]").disabled).toBe(false);
  });

  it("hands the whole answer back, the dispatch with it", async () => {
    const onAssigned = vi.fn();
    call = vi.fn(async () => ({ issue: issue({ id: "issue-1" }), dispatch: { kind: "new_workspace", workspace_id: "ws-9" } }));
    open({ callRpc: call, onAssigned });
    await choose("project_agent");
    press("[data-assign-go]");
    await flush();
    expect(onAssigned.mock.calls[0][0].dispatch.workspace_id).toBe("ws-9");
    handle = null; // the dialog closed itself
  });
});

describe("the new-issue form", () => {
  const open = (over = {}) => {
    handle = openCreateIssue({
      projectId: "proj-1",
      projectName: "Build",
      options: options(),
      catalog: CATALOG,
      callRpc: call,
      ...over,
    });
    return handle;
  };

  const type = (id, value) => {
    const field = document.querySelector(id);
    field.value = value;
    field.dispatchEvent(new Event("input"));
  };

  it("asks for a title, a body, labels, a priority and an assignee", () => {
    open();
    for (const id of ["#issue-new-title", "#issue-new-body", "#issue-new-labels", "#issue-new-priority"]) {
      expect(document.querySelector(id)).not.toBeNull();
    }
    expect(document.querySelector("[data-assignee-select]")).not.toBeNull();
  });

  it("will not file an issue with no title", async () => {
    open();
    press("[data-create-go]");
    await flush();
    expect(call).not.toHaveBeenCalled();
    expect(document.querySelector(".create-error").textContent).toBe("An issue needs a title.");
  });

  // A field nobody filled in is left off: the verb's own defaults are the
  // record's defaults, and an empty string would store one.
  it("sends only what was filled in", async () => {
    open();
    type("#issue-new-title", " Kanban drag does not persist ");
    press("[data-create-go]");
    await flush();
    expect(call).toHaveBeenCalledWith("issues.create", {
      project_id: "proj-1", title: "Kanban drag does not persist",
    });
  });

  it("sends the body, the labels and the priority when they were", async () => {
    open();
    type("#issue-new-title", "Kanban drag");
    type("#issue-new-body", "Dragging a card…");
    type("#issue-new-labels", " bug , ui , bug ");
    const priority = document.querySelector("#issue-new-priority");
    priority.value = "high";
    priority.dispatchEvent(new Event("change"));
    press("[data-create-go]");
    await flush();
    expect(call.mock.calls[0][1]).toEqual({
      project_id: "proj-1", title: "Kanban drag", body: "Dragging a card…",
      labels: ["bug", "ui"], priority: "high",
    });
  });

  // `issues.create` runs the whole of `issues.assign` inside its own
  // transaction, so filing and dispatching is one press.
  it("files and dispatches in one press", async () => {
    open();
    type("#issue-new-title", "Kanban drag");
    await choose("project_agent");
    type("#issue-new-title", "Kanban drag");
    press("[data-create-go]");
    await flush();
    expect(call.mock.calls[0][1].assignee).toEqual({ kind: "project_agent" });
  });

  it("says as much on the press", async () => {
    open();
    expect(document.querySelector("[data-create-go]").textContent).toBe("File issue");
    await choose("new_workspace");
    expect(document.querySelector("[data-create-go]").textContent).toBe("File and start");
  });
});

describe("labels as a person types them", () => {
  it("trims, drops the empties and keeps one of each", () => {
    expect(labelsFromText(" bug , ui ,, bug ")).toEqual(["bug", "ui"]);
    expect(labelsFromText("")).toEqual([]);
  });
});

describe("the create params on their own", () => {
  it("leaves the default priority off, because the verb's default is the record's", () => {
    const state = { projectId: "p1", title: "x", body: "", labels: "", priority: "none" };
    expect(createIssueParams(state, null)).toEqual({ project_id: "p1", title: "x" });
  });
});
