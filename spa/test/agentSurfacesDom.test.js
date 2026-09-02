// @vitest-environment jsdom
import { describe, expect, it, afterEach, beforeEach, vi } from "vitest";
import { coreSourceOf } from "./coreSource.js";
import { mountAgentSurfaces } from "../src/core/agentSurfaces.js";
import {
  AGENT_ENTRY_KIND,
  CHECKLIST_ENTRY_KIND,
  SHELL_ENTRY_KIND,
  SURFACE_PILL_GRACE_MS,
  WORKFLOW_ENTRY_KIND,
  rowActions,
  surfaceRows,
} from "../src/core/agentSurfacesModel.js";

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notify: () => {} }));

const SPAWNING_CALL_SEQUENCE = 10;
const SURFACES_KEY = "branch-1:agent-1";

const snapshot = () => ({
  workflows: [
    {
      id: "wf-1",
      name: "Review sweep",
      state: "running",
      phases: [
        { title: "Read", agents: [{ id: "a1", label: "reader", state: "running" }] },
        { title: "Judge", agents: [{ id: "a2", label: "judge", state: "queued" }] },
      ],
    },
  ],
  subagents: [
    { id: "s1", label: "parser reviewer", state: "done", call_sequence: SPAWNING_CALL_SEQUENCE },
    { id: "s2", label: "fixture writer", state: "running" },
  ],
  shells: [{ id: "sh1", description: "cargo test", state: "running", tail: ["running 12 tests"] }],
  checklist: [{ id: "c1", subject: "Land the fold", state: "in_progress" }],
});

const composerBlock = () => {
  document.body.innerHTML = `<div class="rail-composer">
    <div class="rail-status" id="rail-status"></div>
    <div class="rail-surfaces" id="rail-surfaces"></div>
    <textarea id="railinput"></textarea>
  </div>`;
  return document.getElementById("rail-surfaces");
};

const mount = (options = {}) =>
  mountAgentSurfaces(composerBlock(), {
    key: SURFACES_KEY,
    onSendMessage: options.onSendMessage || (async () => {}),
    onOpenThreadItem: options.onOpenThreadItem || (() => {}),
  });

const pill = (kind) => document.querySelector(`[data-surface-kind="${kind}"]`);
const pressPill = (kind) => pill(kind).click();
const pressed = (kind) => pill(kind).getAttribute("aria-pressed");
const viewerRows = (selector) => [...document.querySelectorAll(`${selector} > .surface-row`)];
const runningRows = () => viewerRows(".surface-running");
const completedRows = () => viewerRows(".surface-completed-rows");
const completedFold = () => document.querySelector(".surface-completed");

const chooseRowAction = (row, actionId) => {
  row.querySelector(".caret").click();
  row.querySelector(`.mi[data-action="${actionId}"]`).click();
};

beforeEach(() => {
  notifyError.mockClear();
  globalThis.localStorage.clear();
});

describe("the surface pills", () => {
  it("paints one pill per kind with content, and toggles one viewer at a time", () => {
    const surfaces = mount();
    surfaces.set(snapshot());

    expect([...document.querySelectorAll(".surface-pill")].map((button) => button.dataset.surfaceKind)).toEqual([
      WORKFLOW_ENTRY_KIND,
      AGENT_ENTRY_KIND,
      SHELL_ENTRY_KIND,
      CHECKLIST_ENTRY_KIND,
    ]);
    expect(document.querySelector(".surface-viewer")).toBe(null);

    pressPill(SHELL_ENTRY_KIND);
    expect(pressed(SHELL_ENTRY_KIND)).toBe("true");
    expect(document.querySelector(".surface-shells")).not.toBe(null);

    pressPill(SHELL_ENTRY_KIND);
    expect(pressed(SHELL_ENTRY_KIND)).toBe("false");
    expect(document.querySelector(".surface-viewer")).toBe(null);

    pressPill(SHELL_ENTRY_KIND);
    pressPill(CHECKLIST_ENTRY_KIND);
    expect(pressed(SHELL_ENTRY_KIND)).toBe("false");
    expect(pressed(CHECKLIST_ENTRY_KIND)).toBe("true");
    expect(document.querySelector(".surface-shells")).toBe(null);
    expect(document.querySelector(".surface-checklist")).not.toBe(null);

    surfaces.dispose();
  });

  it("remembers the open kind through a remount, and opens nothing when it is gone", () => {
    const first = mount();
    first.set(snapshot());
    pressPill(AGENT_ENTRY_KIND);
    first.dispose();

    const second = mount();
    second.set(snapshot());
    expect(pressed(AGENT_ENTRY_KIND)).toBe("true");
    expect(document.querySelector(".surface-subagents")).not.toBe(null);
    second.dispose();

    const third = mount();
    third.set({ shells: snapshot().shells });
    expect(document.querySelector(".surface-viewer")).toBe(null);
    expect(pressed(SHELL_ENTRY_KIND)).toBe("false");
    third.dispose();
  });
});

describe("the pill that lingers after the work stops", () => {
  const finishedShells = () => {
    const finished = snapshot();
    finished.shells = finished.shells.map((shell) => ({ ...shell, state: "done", exit_code: 0 }));
    return finished;
  };

  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("counts the running shells and drops the pill when the grace it armed runs out", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    expect(pill(SHELL_ENTRY_KIND).querySelector(".surface-pill-count").textContent).toBe("1");

    surfaces.set(finishedShells());
    expect(pill(SHELL_ENTRY_KIND).querySelector(".surface-pill-count")).toBe(null);
    expect(pill(SHELL_ENTRY_KIND).textContent.trim()).toBe("Shells");

    vi.advanceTimersByTime(SURFACE_PILL_GRACE_MS - 1);
    expect(pill(SHELL_ENTRY_KIND)).not.toBe(null);

    vi.advanceTimersByTime(1);
    expect(pill(SHELL_ENTRY_KIND)).toBe(null);
    surfaces.dispose();
  });

  it("keeps the pill and the viewer while it is open, and closes both a grace after it is shut", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(SHELL_ENTRY_KIND);
    surfaces.set(finishedShells());

    vi.advanceTimersByTime(SURFACE_PILL_GRACE_MS * 3);
    expect(pressed(SHELL_ENTRY_KIND)).toBe("true");
    expect(document.querySelector(".surface-shells")).not.toBe(null);

    pressPill(SHELL_ENTRY_KIND);
    vi.advanceTimersByTime(SURFACE_PILL_GRACE_MS - 1);
    expect(pill(SHELL_ENTRY_KIND)).not.toBe(null);
    vi.advanceTimersByTime(1);
    expect(pill(SHELL_ENTRY_KIND)).toBe(null);
    expect(document.querySelector(".surface-viewer")).toBe(null);
    surfaces.dispose();
  });

  it("closes the viewer of a kind the grace stopped showing", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(SHELL_ENTRY_KIND);
    surfaces.set(finishedShells());
    pressPill(CHECKLIST_ENTRY_KIND);

    vi.advanceTimersByTime(SURFACE_PILL_GRACE_MS);

    expect(pill(SHELL_ENTRY_KIND)).toBe(null);
    expect(document.querySelector(".surface-checklist")).not.toBe(null);
    surfaces.dispose();
  });

  it("keeps the pill of a kind still running, and arms no timer for it", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    expect(vi.getTimerCount()).toBe(0);

    vi.advanceTimersByTime(SURFACE_PILL_GRACE_MS * 3);
    expect(pill(SHELL_ENTRY_KIND)).not.toBe(null);
    surfaces.dispose();
  });

  it("arms one timer at a time and clears it on dispose", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    surfaces.set(finishedShells());
    expect(vi.getTimerCount()).toBe(1);

    surfaces.set({ ...finishedShells(), checklist: [{ id: "c2", subject: "another", state: "pending" }] });
    expect(vi.getTimerCount()).toBe(1);

    surfaces.dispose();
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("painting the viewer", () => {
  it("keeps the rows, the draft and the focus across a set that changes nothing", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(AGENT_ENTRY_KIND);
    const input = document.getElementById("railinput");
    input.value = "half a sentence";
    input.focus();
    const rowsBefore = [...runningRows(), ...completedRows()];

    surfaces.set(snapshot());

    expect([...runningRows(), ...completedRows()]).toEqual(rowsBefore);
    expect(input.value).toBe("half a sentence");
    expect(document.activeElement).toBe(input);
    surfaces.dispose();
  });

  it("keeps the agent rows and the scroll when a workflow phase gains an agent", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(WORKFLOW_ENTRY_KIND);
    const viewerRegion = document.querySelector(".rail-surfaces-viewer");
    viewerRegion.scrollTop = 40;
    const [firstAgentRow] = viewerRows(".surface-phase-agents");
    expect(firstAgentRow).not.toBe(undefined);

    const grown = snapshot();
    grown.workflows[0].phases[0].agents.push({ id: "a3", label: "second reader", state: "queued" });
    surfaces.set(grown);

    const rowsAfter = viewerRows(".surface-phase-agents");
    expect(rowsAfter).toHaveLength(2);
    expect(rowsAfter[0]).toBe(firstAgentRow);
    expect(viewerRegion.scrollTop).toBe(40);
    surfaces.dispose();
  });

  it("keeps the agent rows and the scroll when the workflow's own head moves", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(WORKFLOW_ENTRY_KIND);
    const viewerRegion = document.querySelector(".rail-surfaces-viewer");
    viewerRegion.scrollTop = 40;
    const [firstAgentRow] = viewerRows(".surface-phase-agents");
    const [firstPhase] = [...document.querySelectorAll(".surface-phase")];

    const finished = snapshot();
    finished.workflows[0].state = "done";
    finished.workflows[0].description = "Read every parser and judge it";
    surfaces.set(finished);

    expect(viewerRows(".surface-phase-agents")[0]).toBe(firstAgentRow);
    expect([...document.querySelectorAll(".surface-phase")][0]).toBe(firstPhase);
    expect(viewerRegion.scrollTop).toBe(40);
    expect(document.querySelector(".surface-workflow-head").textContent).toContain("Read every parser");
    surfaces.dispose();
  });

  it("paints a phase's agents without rebuilding them when another phase is chosen", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(WORKFLOW_ENTRY_KIND);
    const [firstPhase, secondPhase] = [...document.querySelectorAll(".surface-phase")];

    secondPhase.click();

    expect(secondPhase.getAttribute("aria-pressed")).toBe("true");
    expect(firstPhase.getAttribute("aria-pressed")).toBe("false");
    expect(viewerRows(".surface-phase-agents").map((row) => row.textContent)).toHaveLength(1);
    expect(document.querySelector(".surface-phase-agents").textContent).toContain("judge");
    surfaces.dispose();
  });

  it("shows one workflow at a time, and offers no chooser while there is only one", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(WORKFLOW_ENTRY_KIND);

    expect(document.querySelectorAll(".surface-workflow-choice")).toHaveLength(0);
    expect(document.querySelector(".surface-workflow-head").textContent).toContain("Review sweep");
    surfaces.dispose();
  });

  it("lets the reader reach the second workflow the pill counted", () => {
    const surfaces = mount();
    const both = snapshot();
    both.workflows.push({
      id: "wf-2",
      name: "Fixture sweep",
      state: "running",
      phases: [{ title: "Write", agents: [{ id: "b1", label: "fixture writer", state: "running" }] }],
    });
    surfaces.set(both);
    pressPill(WORKFLOW_ENTRY_KIND);

    expect(pill(WORKFLOW_ENTRY_KIND).querySelector(".surface-pill-count").textContent).toBe("2");
    expect(pill(CHECKLIST_ENTRY_KIND).querySelector(".surface-pill-count").textContent).toBe("1");
    const choices = [...document.querySelectorAll(".surface-workflow-choice")];
    expect(choices).toHaveLength(2);
    expect(choices[0].getAttribute("aria-pressed")).toBe("true");

    choices[1].click();

    expect(choices[1].getAttribute("aria-pressed")).toBe("true");
    expect(choices[0].getAttribute("aria-pressed")).toBe("false");
    expect(document.querySelector(".surface-workflow-head").textContent).toContain("Fixture sweep");
    expect(document.querySelector(".surface-phase-agents").textContent).toContain("fixture writer");
    surfaces.dispose();
  });

  it("falls back to the first workflow when the chosen one leaves the snapshot", () => {
    const surfaces = mount();
    const both = snapshot();
    both.workflows.push({ id: "wf-2", name: "Fixture sweep", state: "running", phases: [] });
    surfaces.set(both);
    pressPill(WORKFLOW_ENTRY_KIND);
    [...document.querySelectorAll(".surface-workflow-choice")][1].click();

    surfaces.set(snapshot());

    expect(document.querySelector(".surface-workflow-head").textContent).toContain("Review sweep");
    expect(document.querySelectorAll(".surface-workflow-choice")).toHaveLength(0);
    surfaces.dispose();
  });

  it("paints entries with missing or repeated ids rather than throwing", () => {
    const surfaces = mount();
    surfaces.set({
      shells: [{ description: "first", state: "running" }, { description: "second", state: "running" }],
      subagents: [
        { id: "same", label: "one", state: "running" },
        { id: "same", label: "two", state: "running" },
      ],
    });

    pressPill(SHELL_ENTRY_KIND);
    expect(runningRows().map((row) => row.dataset.key)).toEqual(["shells-0", "shells-1"]);

    pressPill(AGENT_ENTRY_KIND);
    expect(runningRows().map((row) => row.dataset.key)).toEqual(["same", "agent-1"]);
    surfaces.dispose();
  });

  it("keeps a fold the reader opened across a repaint", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(SHELL_ENTRY_KIND);
    const fold = document.querySelector(".surface-shell-tail");
    fold.open = true;

    const moved = snapshot();
    moved.shells[0].tail = ["running 12 tests", "test parser::folds ... ok"];
    surfaces.set(moved);

    expect(document.querySelector(".surface-shell-tail")).toBe(fold);
    expect(fold.open).toBe(true);
    surfaces.dispose();
  });
});

describe("a row's action", () => {
  it("sends exactly the message rowActions named, once", async () => {
    const onSendMessage = vi.fn(async () => {});
    const surfaces = mount({ onSendMessage });
    surfaces.set(snapshot());
    pressPill(AGENT_ENTRY_KIND);
    const [row] = completedRows();
    const [action] = rowActions(AGENT_ENTRY_KIND, surfaceRows(AGENT_ENTRY_KIND, snapshot())[0]);

    chooseRowAction(row, action.id);

    expect(onSendMessage.mock.calls).toEqual([[action.message]]);
    surfaces.dispose();
  });

  it("says a refused send failed and leaves the viewer where it was", async () => {
    const onSendMessage = vi.fn(async () => {
      throw new Error("no agent is listening");
    });
    const surfaces = mount({ onSendMessage });
    surfaces.set(snapshot());
    pressPill(CHECKLIST_ENTRY_KIND);
    const [row] = viewerRows(".surface-checklist");
    const [action] = rowActions(CHECKLIST_ENTRY_KIND, surfaceRows(CHECKLIST_ENTRY_KIND, snapshot())[0]);

    chooseRowAction(row, action.id);
    await vi.waitFor(() => expect(notifyError).toHaveBeenCalled());

    expect(notifyError.mock.calls[0][1]).toBe("no agent is listening");
    expect(pressed(CHECKLIST_ENTRY_KIND)).toBe("true");
    expect(document.querySelector(".surface-checklist")).not.toBe(null);
    surfaces.dispose();
  });

  it("leaves a menu the reader opened open across a repaint, still choosing the same action", () => {
    const onSendMessage = vi.fn(async () => {});
    const surfaces = mount({ onSendMessage });
    surfaces.set(snapshot());
    pressPill(SHELL_ENTRY_KIND);
    const [row] = runningRows();
    row.querySelector(".caret").click();

    surfaces.set(snapshot());

    expect(row.querySelector(".splitmenu").hidden).toBe(false);
    const [action] = rowActions(SHELL_ENTRY_KIND, surfaceRows(SHELL_ENTRY_KIND, snapshot())[0]);
    row.querySelector(`.mi[data-action="${action.id}"]`).click();
    expect(onSendMessage.mock.calls).toEqual([[action.message]]);
    surfaces.dispose();
  });

  it("says so when the row a menu was opened on has left the snapshot", () => {
    const onSendMessage = vi.fn(async () => {});
    const surfaces = mount({ onSendMessage });
    surfaces.set(snapshot());
    pressPill(SHELL_ENTRY_KIND);
    const [row] = runningRows();
    row.querySelector(".caret").click();

    const replaced = snapshot();
    replaced.shells = [{ id: "sh2", description: "cargo clippy", state: "running", tail: [] }];
    surfaces.set(replaced);
    row.querySelector(".mi").click();

    expect(onSendMessage).not.toHaveBeenCalled();
    expect(notifyError).toHaveBeenCalledTimes(1);
    surfaces.dispose();
  });

  it("never reaches the bridge itself", () => {
    const source = coreSourceOf("agentSurfaces.js");

    expect(source).not.toContain("App.call");
    expect(source).not.toContain("../app.js");
    expect(source).not.toContain("setInterval");
  });
});

describe("a menu the paint had to rebuild", () => {
  it("works on the workflow head from the very first paint", () => {
    const onSendMessage = vi.fn(async () => {});
    const surfaces = mount({ onSendMessage });
    surfaces.set(snapshot());
    pressPill(WORKFLOW_ENTRY_KIND);

    const head = document.querySelector(".surface-workflow-head");
    const [action] = rowActions(WORKFLOW_ENTRY_KIND, snapshot().workflows[0]);
    head.querySelector(".caret").click();

    expect(head.querySelector(".splitmenu").hidden).toBe(false);
    head.querySelector(`.mi[data-action="${action.id}"]`).click();
    expect(onSendMessage.mock.calls).toEqual([[action.message]]);
    surfaces.dispose();
  });

  it("still chooses an action on the row a finished shell was redrawn as inside the fold", () => {
    const onSendMessage = vi.fn(async () => {});
    const surfaces = mount({ onSendMessage });
    surfaces.set(snapshot());
    pressPill(SHELL_ENTRY_KIND);
    const [wasRunning] = runningRows();

    const finished = snapshot();
    finished.shells[0] = { ...finished.shells[0], state: "done", exit_code: 0 };
    surfaces.set(finished);

    expect(runningRows()).toEqual([]);
    const [row] = completedRows();
    expect(row).not.toBe(wasRunning);
    const [action] = rowActions(SHELL_ENTRY_KIND, surfaceRows(SHELL_ENTRY_KIND, finished)[0]);
    chooseRowAction(row, action.id);

    expect(onSendMessage.mock.calls).toEqual([[action.message]]);
    surfaces.dispose();
  });

  it("still chooses an action after a subagent's model gives the row a new trailing slot", () => {
    const onSendMessage = vi.fn(async () => {});
    const surfaces = mount({ onSendMessage });
    surfaces.set(snapshot());
    pressPill(AGENT_ENTRY_KIND);
    const [row] = completedRows();

    const named = snapshot();
    named.subagents[0] = { ...named.subagents[0], model: "haiku" };
    surfaces.set(named);

    expect(completedRows()[0]).toBe(row);
    const [action] = rowActions(AGENT_ENTRY_KIND, surfaceRows(AGENT_ENTRY_KIND, named)[0]);
    chooseRowAction(row, action.id);

    expect(onSendMessage.mock.calls).toEqual([[action.message]]);
    surfaces.dispose();
  });
});

describe("a set that would change nothing", () => {
  it("paints nothing at all", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(SHELL_ENTRY_KIND);
    const [row] = runningRows();
    const marker = document.createElement("span");
    marker.className = "paint-witness";
    row.appendChild(marker);

    surfaces.set(snapshot());

    expect(row.querySelector(".paint-witness")).toBe(marker);
    surfaces.dispose();
  });
});

describe("pressing a subagent row", () => {
  it("opens the thread item the row was spawned by, and only for a row that names one", () => {
    const onOpenThreadItem = vi.fn();
    const surfaces = mount({ onOpenThreadItem });
    surfaces.set(snapshot());
    pressPill(AGENT_ENTRY_KIND);
    const [spawned] = completedRows();
    const [unspawned] = runningRows();

    spawned.click();
    expect(onOpenThreadItem.mock.calls).toEqual([[SPAWNING_CALL_SEQUENCE]]);

    unspawned.click();
    expect(onOpenThreadItem).toHaveBeenCalledTimes(1);
    surfaces.dispose();
  });
});

describe("the fold the finished rows sit under", () => {
  it("holds every finished row and counts them, leaving the running ones above it", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(AGENT_ENTRY_KIND);

    expect(runningRows().map((row) => row.dataset.key)).toEqual(["s2"]);
    expect(completedRows().map((row) => row.dataset.key)).toEqual(["s1"]);
    expect(completedFold().querySelector(".surface-completed-head").textContent.trim()).toBe("Completed (1)");
    surfaces.dispose();
  });

  it("is not there at all while nothing has finished", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(SHELL_ENTRY_KIND);

    expect(runningRows()).toHaveLength(1);
    expect(completedFold()).toBe(null);
    surfaces.dispose();
  });

  it("appears when the first row finishes and goes when the last finished row leaves", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(SHELL_ENTRY_KIND);

    const finished = snapshot();
    finished.shells[0] = { ...finished.shells[0], state: "done" };
    surfaces.set(finished);
    expect(completedFold()).not.toBe(null);
    expect(completedRows().map((row) => row.dataset.key)).toEqual(["sh1"]);

    surfaces.set(snapshot());
    expect(completedFold()).toBe(null);
    expect(runningRows().map((row) => row.dataset.key)).toEqual(["sh1"]);
    surfaces.dispose();
  });

  it("keeps the fold the reader opened, and the rows under it, across a repaint", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(AGENT_ENTRY_KIND);
    const fold = completedFold();
    fold.open = true;
    const [finishedRow] = completedRows();

    const moved = snapshot();
    moved.subagents[1] = { ...moved.subagents[1], model: "haiku" };
    surfaces.set(moved);

    expect(completedFold()).toBe(fold);
    expect(fold.open).toBe(true);
    expect(completedRows()[0]).toBe(finishedRow);
    surfaces.dispose();
  });

  it("counts the fold again when another row finishes", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(AGENT_ENTRY_KIND);

    const both = snapshot();
    both.subagents[1] = { ...both.subagents[1], state: "done" };
    surfaces.set(both);

    expect(completedFold().querySelector(".surface-completed-head").textContent.trim()).toBe("Completed (2)");
    expect(completedRows().map((row) => row.dataset.key)).toEqual(["s1", "s2"]);
    expect(runningRows()).toEqual([]);
    surfaces.dispose();
  });
});
