/** @vitest-environment jsdom */
// The assignee picker and the inline issue composer — the two surfaces that
// ask the same question with the same control.
//
// Assigning IS dispatching, so the control says what the chosen option is about
// to do before it is pressed, and the two creating kinds are the only ones that
// open the harness/model/effort selects.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { assignRefusalText, assigneeOptions, workspaceAgents } from "../src/core/trackerAssignee.js";
import { openAssigneePicker } from "../src/core/trackerAssigneePicker.js";
import { attachmentsWentNowhere, composedIssueParams, openIssueComposer } from "../src/core/issueComposer.js";
import { labelsFromText } from "../src/core/trackerModel.js";
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

describe("the inline issue composer", () => {
  let slot;

  const open = (over = {}) => {
    slot = document.createElement("div");
    document.body.append(slot);
    handle = openIssueComposer(slot, {
      projectId: "proj-1",
      projectName: "Build",
      columns: null,
      labels: ["bug", "ui"],
      options: options(),
      catalog: CATALOG,
      attachable: true,
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

  /** One of the composer's own menus, by the filter it writes. */
  const menu = (name) => document.querySelector(`[data-filter-menu="${name}"]`);
  const pickInMenu = (name, value) => {
    menu(name).querySelector(".fmenu-press").click();
    const row = [...menu(name).querySelectorAll(".fmenu-row")].find((one) => one.dataset.value === value);
    row.click();
  };

  it("asks for a title, a body, a column, a priority, labels and an assignee", () => {
    open();
    expect(document.querySelector("#issue-new-title")).not.toBeNull();
    expect(document.querySelector("#issue-new-body")).not.toBeNull();
    for (const name of ["status", "priority", "labels"]) expect(menu(name)).not.toBeNull();
    expect(document.querySelector("[data-assignee-select]")).not.toBeNull();
  });

  // #57: the same paperclip, paste and drop the chat composer has.
  it("takes files, with the conversation's own tray", () => {
    open();
    expect(document.querySelector(".issue-compose .composer.attachable")).not.toBeNull();
    expect(document.querySelector(".composer-attach")).not.toBeNull();
    expect(document.querySelector(".composer-dropmask")).not.toBeNull();
  });

  // A press that cannot work is worse than no press: a bridge with no
  // `issues.attach` gets the plain box, not one that apologises afterwards.
  it("offers no paperclip at all against a bridge that cannot carry files", () => {
    open({ attachable: false });
    expect(document.querySelector(".composer-attach")).toBeNull();
    expect(document.querySelector(".composer-tray")).toBeNull();
    expect(document.querySelector(".composer-dropmask")).toBeNull();
    expect(document.querySelector(".issue-compose .composer.attachable")).toBeNull();
    // And the rest of the form is untouched — this is one affordance gone,
    // not a degraded composer.
    expect(document.querySelector("#issue-new-title")).not.toBeNull();
    expect(document.querySelector("#issue-new-body")).not.toBeNull();
  });

  it("still files, and carries no attachments, without the tray", async () => {
    open({ attachable: false });
    type("#issue-new-title", "Kanban drag");
    press("[data-compose-file]");
    await flush();
    expect(call.mock.calls[0][1]).toEqual({ project_id: "proj-1", title: "Kanban drag" });
  });

  it("cancels without a confirm when there is nothing to throw away", async () => {
    open({ attachable: false });
    slot.querySelector(".issue-compose").dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    );
    await flush();
    expect(slot.querySelector(".issue-compose")).toBeNull();
  });

  it("will not file an issue with no title", async () => {
    open();
    press("[data-compose-file]");
    await flush();
    expect(call).not.toHaveBeenCalled();
    expect(document.querySelector(".issue-compose-error").textContent).toBe("An issue needs a title.");
  });

  // A field nobody filled in is left off: the verb's own defaults are the
  // record's defaults, and an empty string would store one.
  it("sends only what was filled in", async () => {
    open();
    type("#issue-new-title", " Kanban drag does not persist ");
    press("[data-compose-file]");
    await flush();
    expect(call).toHaveBeenCalledWith("issues.create", {
      project_id: "proj-1", title: "Kanban drag does not persist",
    });
  });

  it("sends the body, the labels, the priority and the column when they were", async () => {
    open();
    type("#issue-new-title", "Kanban drag");
    type("#issue-new-body", "Dragging a card…");
    pickInMenu("labels", "bug");
    pickInMenu("labels", "ui");
    pickInMenu("priority", "high");
    pickInMenu("status", "ready");
    press("[data-compose-file]");
    await flush();
    expect(call.mock.calls[0][1]).toEqual({
      project_id: "proj-1", title: "Kanban drag", body: "Dragging a card…",
      labels: ["bug", "ui"], priority: "high", status: "ready",
    });
  });

  // A filter chooses from what is; a composer has to be able to name a label
  // nobody has used yet, which is most of what labelling a new issue is.
  it("invents a label that is not on the list yet", async () => {
    open();
    type("#issue-new-title", "Kanban drag");
    menu("labels").querySelector(".fmenu-press").click();
    const search = menu("labels").querySelector(".fmenu-search");
    search.value = "kanban";
    search.dispatchEvent(new Event("input"));
    const coined = [...menu("labels").querySelectorAll(".fmenu-row")].at(-1);
    expect(coined.textContent).toContain("Create");
    coined.click();
    press("[data-compose-file]");
    await flush();
    expect(call.mock.calls[0][1].labels).toEqual(["kanban"]);
  });

  // `issues.create` runs the whole of `issues.assign` inside its own
  // transaction, so filing and dispatching is one press.
  it("files and dispatches in one press", async () => {
    open();
    type("#issue-new-title", "Kanban drag");
    await choose("project_agent");
    press("[data-compose-file]");
    await flush();
    expect(call.mock.calls[0][1].assignee).toEqual({ kind: "project_agent" });
  });

  it("says as much on the press", async () => {
    open();
    expect(document.querySelector("[data-compose-file]").textContent).toBe("File issue");
    await choose("new_workspace");
    expect(document.querySelector("[data-compose-file]").textContent).toBe("File and start");
  });

  // The assignee zone is the one part that is redrawn, and it must not be
  // redrawn under a caret: typing a workspace name keeps the field it is
  // being typed into.
  it("keeps the workspace-name field while it is being typed in", async () => {
    open();
    await choose("new_workspace");
    const name = document.querySelector("#issue-new-name");
    name.focus();
    name.value = "kanban-fix";
    name.dispatchEvent(new Event("input"));
    await flush();
    expect(document.querySelector("#issue-new-name")).toBe(name);
    expect(document.activeElement).toBe(name);
  });

  // #57's keys. Enter in a one-line field means "done with this line", and the
  // next line is the description — a title is rarely the whole issue.
  it("moves from the title to the body on enter, and does not file", async () => {
    open();
    type("#issue-new-title", "Kanban drag");
    const title = document.querySelector("#issue-new-title");
    title.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await flush();
    expect(document.activeElement).toBe(document.querySelector("#issue-new-body"));
    expect(call).not.toHaveBeenCalled();
  });

  it("files on cmd/ctrl+enter, from either field", async () => {
    for (const [field, modifier] of [["#issue-new-title", "metaKey"], ["#issue-new-body", "ctrlKey"]]) {
      call = vi.fn(async () => ({ issue: issue({ id: "issue-1" }) }));
      open({ callRpc: call });
      type("#issue-new-title", "Kanban drag");
      document.querySelector(field).dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", [modifier]: true, bubbles: true, cancelable: true }),
      );
      await flush();
      expect(call.mock.calls[0][0]).toBe("issues.create");
      handle = null;
    }
  });

  // Escape throws the draft away, so it asks — but only when there is a draft
  // to throw. Confirming that you typed nothing is the confirm nobody reads.
  it("shuts on escape with an empty form, and asks first once there is text", async () => {
    open();
    slot.querySelector(".issue-compose").dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    );
    await flush();
    expect(slot.querySelector(".issue-compose")).toBeNull();

    open();
    type("#issue-new-title", "Kanban drag");
    slot.querySelector(".issue-compose").dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    );
    await flush();
    // Still there, behind a confirm that has not been answered.
    expect(slot.querySelector(".issue-compose")).not.toBeNull();
    expect(document.querySelector("[data-confirm-ok]")).not.toBeNull();
    document.querySelector("[data-confirm-ok]").click();
    await flush();
    expect(slot.querySelector(".issue-compose")).toBeNull();
  });

  it("reads the issue out of either answer shape", async () => {
    for (const answer of [
      { issue: issue({ id: "issue-1" }) },
      { issue: issue({ id: "issue-1" }), dispatch: null },
      { issue: issue({ id: "issue-1" }), dispatch: { kind: "new_workspace", workspace_id: "ws-9" } },
    ]) {
      const onFiled = vi.fn();
      call = vi.fn(async () => answer);
      open({ callRpc: call, onFiled });
      type("#issue-new-title", "Kanban drag");
      press("[data-compose-file]");
      await flush();
      expect(onFiled.mock.calls[0][0].issue.id).toBe("issue-1");
      handle = null; // the composer closed itself on success
    }
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
    const draft = { title: "x", body: "", labels: [], priority: [], status: [] };
    expect(composedIssueParams(draft, { projectId: "p1" })).toEqual({ project_id: "p1", title: "x" });
  });

  // #57: the files ride as the same `{path, name}` the thread's own
  // attachments do, so one shape carries them everywhere.
  it("carries the attachments that went up, and none when there were none", () => {
    const draft = { title: "x", labels: [], priority: [], status: [] };
    const files = [{ path: ".build/attachments/abc-shot.png", name: "shot.png" }];
    expect(composedIssueParams(draft, { projectId: "p1", attachments: files }).attachments).toEqual(files);
    expect(composedIssueParams(draft, { projectId: "p1", attachments: [] })).toEqual({ project_id: "p1", title: "x" });
  });
});

// `issues.assign` with `new_workspace` defers: cutting a checkout is real git,
// and the agent cannot exist until it is ready. The reply shape is unchanged —
// every id filled in, no pending placeholder — but the wait is seconds to
// minutes, where every other kind answers in milliseconds.
describe("the one kind that makes you wait", () => {
  /** A call that never settles, so the dialog can be read mid-flight. */
  const held = () => {
    let settle;
    const call = vi.fn(() => new Promise((resolve) => { settle = resolve; }));
    return { call, settle: (answer) => settle(answer) };
  };

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

  const pressText = () => document.querySelector("[data-assign-go]").textContent;

  it("says what it is waiting on rather than just that it is busy", async () => {
    const wire = held();
    open({ callRpc: wire.call });
    await choose("new_workspace");
    press("[data-assign-go]");
    await flush();
    expect(pressText()).toBe("cutting the workspace…");
    expect(document.querySelector(".issue-assign-waiting").textContent)
      .toContain("This can take a minute on a large repository");
  });

  // Every other kind is a write and a lookup; nobody reads its label.
  it("says only that it is assigning for the kinds that answer at once", async () => {
    const wire = held();
    open({ callRpc: wire.call });
    await choose("project_agent");
    press("[data-assign-go]");
    await flush();
    expect(pressText()).toBe("assigning…");
    expect(document.querySelector(".issue-assign-waiting")).toBeNull();
  });

  it("says nothing about waiting before the press", async () => {
    open();
    await choose("new_workspace");
    expect(pressText()).toBe("Assign and start");
    expect(document.querySelector(".issue-assign-waiting")).toBeNull();
  });

  // A failed cut writes nothing at all — the issue stays unassigned, in its old
  // column, with no events — so there is nothing to reconcile and pressing
  // again IS the retry.
  it("keeps the draft and the dialog when the cut is refused", async () => {
    call = vi.fn(async () => {
      throw new Error("another filesystem operation is still running");
    });
    open({ callRpc: call });
    await choose("new_workspace");
    press("[data-assign-go]");
    await flush();
    expect(document.querySelector("[data-assignee-select]").value).toBe("new_workspace");
    expect(document.querySelector("[data-assign-go]").disabled).toBe(false);
    expect(pressText()).toBe("Assign and start");
  });

  it("puts that refusal in words the reader can act on", () => {
    expect(assignRefusalText("another filesystem operation is still running"))
      .toBe("This machine is busy with another checkout. Nothing was assigned — try again in a moment.");
  });

  // Everything else the bridge says is written for a reader already.
  it("hands every other refusal over in the bridge's own words", () => {
    expect(assignRefusalText("agent agent-1 is not in project proj-1"))
      .toBe("agent agent-1 is not in project proj-1");
  });
});

// Three things the picker must NOT do, which the dispatch's shape makes easy
// to get wrong later.
describe("what the picker leaves to the issue", () => {
  const openOn = (over = {}) => {
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

  it("never reads workspace_id off the dispatch", async () => {
    // It is null for every kind but `new_workspace` — only set when the
    // dispatch MADE the workspace. What a surface wants is
    // `issue.links.workspace_ids`, which records what was made OR used.
    const onAssigned = vi.fn();
    call = vi.fn(async () => ({
      issue: issue({ id: "issue-1" }),
      dispatch: { kind: "agent", workspace_id: null, entity_id: "run-1", agent_id: "agent-1", operation_id: "op-1" },
    }));
    openOn({ callRpc: call, onAssigned });
    await choose("agent:agent-1");
    press("[data-assign-go]");
    await flush();
    // The whole answer goes back untouched; nothing here reads into it.
    expect(onAssigned.mock.calls[0][0].dispatch.workspace_id).toBeNull();
    handle = null;
  });

  it("sends no status of its own, so a dispatch moves the card or does not", async () => {
    // A dispatch moves an issue to In progress only from Backlog or Ready.
    // The picker never says where the card should land — it assigns, and the
    // re-read says where the issue ended up.
    openOn();
    await choose("project_agent");
    press("[data-assign-go]");
    await flush();
    expect(call.mock.calls[0][1]).not.toHaveProperty("status");
  });
});

// A v1 handler parses params into its own struct and serialises THAT back
// before the implementation reads them, so a field the bridge predates is
// dropped at the facade rather than refused. Filing with an assignee — or with
// files (#57) — on a bridge that does not take them answers ok with neither:
// no error, and nothing in the answer that says so.
describe("what a bridge dropped on the floor", () => {
  const filed = (over = {}) => ({ issue: issue({ id: "issue-1", number: 12, ...over }) });
  const FILES = [{ path: ".build/attachments/abc-shot.png", name: "shot.png" }];

  const fileWith = async (over, onFiled) => {
    const slot = document.createElement("div");
    document.body.append(slot);
    handle = openIssueComposer(slot, {
      projectId: "proj-1", projectName: "Build", columns: null, labels: [],
      options: options(), catalog: CATALOG, callRpc: call, onFiled, ...over,
    });
    const title = document.querySelector("#issue-new-title");
    title.value = "Kanban drag";
    title.dispatchEvent(new Event("input"));
    await choose("project_agent");
    document.querySelector("[data-compose-file]").click();
    await flush();
    handle = null;
  };

  it("reads the answer against the request", () => {
    // Asked for files, came back with none: dropped.
    expect(attachmentsWentNowhere(FILES, filed())).toBe(true);
    // Asked for files and got them: fine.
    expect(attachmentsWentNowhere(FILES, filed({ attachments: FILES }))).toBe(false);
    // Sent none: an issue with no files is what was wanted.
    expect(attachmentsWentNowhere([], filed())).toBe(false);
    expect(attachmentsWentNowhere(null, filed())).toBe(false);
  });

  // Assignment is dispatch, so a dropped assignee is work the reader believes
  // has started and has not. That is the worst thing to leave unsaid.
  it("says so after the file, rather than in a form that already succeeded", async () => {
    const onFiled = vi.fn();
    call = vi.fn(async () => filed());
    await fileWith({ callRpc: call }, onFiled);
    expect(onFiled.mock.calls[0][1]).toMatchObject({ assigneeWentNowhere: true, attachmentsWentNowhere: false });
  });

  it("says nothing when the assignee landed", async () => {
    const onFiled = vi.fn();
    call = vi.fn(async () => filed({ assignee: { kind: "project_agent" } }));
    await fileWith({ callRpc: call }, onFiled);
    expect(onFiled.mock.calls[0][1]).toMatchObject({ assigneeWentNowhere: false });
  });
});
