// @vitest-environment jsdom
// The pills under the composer, and the viewer above them.
//
// The region is mounted once and painted from a digest, so the composer beside
// it is never rebuilt to say a workflow gained an agent: the draft and the
// caret in it survive every repaint, and so do the rows the reader is looking
// at.

import { describe, expect, it, beforeEach, vi } from "vitest";
import { coreSourceOf } from "./coreSource.js";
import { mountAgentSurfaces } from "../src/core/agentSurfaces.js";
import {
  AGENT_ENTRY_KIND,
  CHECKLIST_ENTRY_KIND,
  SHELL_ENTRY_KIND,
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

describe("painting the viewer", () => {
  it("keeps the rows, the draft and the focus across a set that changes nothing", () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    pressPill(AGENT_ENTRY_KIND);
    const input = document.getElementById("railinput");
    input.value = "half a sentence";
    input.focus();
    const rowsBefore = viewerRows(".surface-subagents");

    surfaces.set(snapshot());

    expect(viewerRows(".surface-subagents")).toEqual(rowsBefore);
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
      shells: [{ description: "first" }, { description: "second" }],
      subagents: [{ id: "same", label: "one" }, { id: "same", label: "two" }],
    });

    pressPill(SHELL_ENTRY_KIND);
    expect(viewerRows(".surface-shells").map((row) => row.dataset.key)).toEqual(["shells-0", "shells-1"]);

    pressPill(AGENT_ENTRY_KIND);
    expect(viewerRows(".surface-subagents").map((row) => row.dataset.key)).toEqual(["same", "agent-1"]);
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
    const [row] = viewerRows(".surface-subagents");
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
    const [row] = viewerRows(".surface-shells");
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
    const [row] = viewerRows(".surface-shells");
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

describe("pressing a subagent row", () => {
  it("opens the thread item the row was spawned by, and only for a row that names one", () => {
    const onOpenThreadItem = vi.fn();
    const surfaces = mount({ onOpenThreadItem });
    surfaces.set(snapshot());
    pressPill(AGENT_ENTRY_KIND);
    const [spawned, unspawned] = viewerRows(".surface-subagents");

    spawned.click();
    expect(onOpenThreadItem.mock.calls).toEqual([[SPAWNING_CALL_SEQUENCE]]);

    unspawned.click();
    expect(onOpenThreadItem).toHaveBeenCalledTimes(1);
    surfaces.dispose();
  });
});
