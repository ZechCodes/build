// @vitest-environment jsdom
import { describe, expect, it, afterEach, beforeEach, vi } from "vitest";
import { coreSourceOf } from "./coreSource.js";
import { SPAWNING_CALL_SEQUENCE, surfacesSnapshot } from "./surfacesFixture.js";
import { motionSettled } from "../src/core/motion.js";
import { EXITING_ATTRIBUTE } from "../src/core/patchList.js";
import { mountAgentSurfaces } from "../src/core/agentSurfaces.js";
import {
  AGENT_ENTRY_KIND,
  CHECKLIST_ENTRY_KIND,
  SHELL_ENTRY_KIND,
  SURFACE_PILL_GRACE_MS,
  WORKFLOW_ENTRY_KIND,
} from "../src/core/agentSurfacesModel.js";

const SURFACES_KEY = "branch-1:agent-1";

const snapshot = () => surfacesSnapshot();

const conversationColumn = () => {
  document.body.innerHTML = `<div class="rail-panel">
    <div class="rail-body" id="rail-body"></div>
    <div class="rail-composer">
      <div class="rail-surfaces-viewer" id="rail-surfaces-viewer" hidden></div>
      <div class="rail-status" id="rail-status">
        <span class="rail-status-lead" id="rail-status-lead"></span>
        <div class="rail-status-pills" id="rail-status-pills"></div>
        <span class="rail-status-git" id="rail-status-git"></span>
      </div>
      <textarea id="railinput"></textarea>
    </div>
  </div>`;
  return {
    pillHost: document.getElementById("rail-status-pills"),
    viewerHost: document.getElementById("rail-surfaces-viewer"),
  };
};

const mount = (options = {}) =>
  mountAgentSurfaces({
    ...conversationColumn(),
    key: SURFACES_KEY,
    onOpenThreadItem: options.onOpenThreadItem || (() => {}),
  });

const pill = (kind) => document.querySelector(`[data-surface-kind="${kind}"]`);
const pressPill = async (kind) => {
  pill(kind).click();
  await motionSettled();
};
const pillCount = (kind) => {
  const cap = pill(kind).querySelector(".surface-pill-count");
  return cap.hidden ? null : cap.textContent;
};
const pressed = (kind) => pill(kind).getAttribute("aria-pressed");
const standing = (selector) => [...document.querySelectorAll(`${selector}:not([${EXITING_ATTRIBUTE}])`)];
const viewerRows = (selector) => standing(`${selector} > .surface-row`);
const runningRows = () => viewerRows(".surface-running");
const completedRows = () => viewerRows(".surface-completed-rows");
const completedFold = () => document.querySelector(".surface-completed");
const historyToggle = () => document.querySelector(".surface-history-toggle");
const phaseSections = () => standing(".surface-phase");
const agentRowsIn = (section) => [...section.querySelectorAll(`.surface-row:not([${EXITING_ATTRIBUTE}])`)];
const agentLabelsIn = (section) =>
  agentRowsIn(section).map((row) => row.querySelector(".surface-row-label").textContent);

beforeEach(() => globalThis.localStorage.clear());

describe("the surface pills", () => {
  it("paints one pill per kind with content, and toggles one viewer at a time", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());

    expect([...document.querySelectorAll(".surface-pill")].map((button) => button.dataset.surfaceKind)).toEqual([
      WORKFLOW_ENTRY_KIND,
      AGENT_ENTRY_KIND,
      SHELL_ENTRY_KIND,
      CHECKLIST_ENTRY_KIND,
    ]);
    expect(document.querySelector(".surface-viewer")).toBe(null);

    await pressPill(SHELL_ENTRY_KIND);
    expect(pressed(SHELL_ENTRY_KIND)).toBe("true");
    expect(document.querySelector(".surface-shells")).not.toBe(null);

    await pressPill(SHELL_ENTRY_KIND);
    expect(pressed(SHELL_ENTRY_KIND)).toBe("false");
    expect(document.querySelector(".surface-viewer")).toBe(null);

    await pressPill(SHELL_ENTRY_KIND);
    await pressPill(CHECKLIST_ENTRY_KIND);
    expect(pressed(SHELL_ENTRY_KIND)).toBe("false");
    expect(pressed(CHECKLIST_ENTRY_KIND)).toBe("true");
    expect(document.querySelector(".surface-shells")).toBe(null);
    expect(document.querySelector(".surface-checklist")).not.toBe(null);

    surfaces.dispose();
  });

  it("dismisses the selected kind with Escape and returns focus to its footer control", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(SHELL_ENTRY_KIND);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await motionSettled();

    expect(pressed(SHELL_ENTRY_KIND)).toBe("false");
    expect(document.querySelector(".surface-viewer")).toBe(null);
    expect(document.activeElement).toBe(pill(SHELL_ENTRY_KIND));
    surfaces.dispose();
  });

  it("remembers the open kind through a remount, and opens nothing when it is gone", async () => {
    const first = mount();
    first.set(snapshot());
    await pressPill(AGENT_ENTRY_KIND);
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

  it("counts the running shells and drops the pill when the grace it armed runs out", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    expect(pillCount(SHELL_ENTRY_KIND)).toBe("1");

    surfaces.set(finishedShells());
    expect(pillCount(SHELL_ENTRY_KIND)).toBe(null);
    expect(pill(SHELL_ENTRY_KIND).textContent.trim()).toBe("Shells");

    await vi.advanceTimersByTimeAsync(SURFACE_PILL_GRACE_MS - 1);
    expect(pill(SHELL_ENTRY_KIND)).not.toBe(null);

    await vi.advanceTimersByTimeAsync(1);
    expect(pill(SHELL_ENTRY_KIND)).toBe(null);
    surfaces.dispose();
  });

  it("lingers from when the snapshot was seen, not from when it was handed over", async () => {
    const surfaces = mount();
    surfaces.set(snapshot(), Date.now() - SURFACE_PILL_GRACE_MS - 1);
    expect(pill(SHELL_ENTRY_KIND)).not.toBe(null);

    surfaces.set(finishedShells());
    expect(standing(`[data-surface-kind="${SHELL_ENTRY_KIND}"]`)).toEqual([]);
    surfaces.dispose();
  });

  it("keeps the pill and the viewer while it is open, and closes both a grace after it is shut", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(SHELL_ENTRY_KIND);
    surfaces.set(finishedShells());

    await vi.advanceTimersByTimeAsync(SURFACE_PILL_GRACE_MS * 3);
    expect(pressed(SHELL_ENTRY_KIND)).toBe("true");
    expect(document.querySelector(".surface-shells")).not.toBe(null);

    await pressPill(SHELL_ENTRY_KIND);
    await vi.advanceTimersByTimeAsync(SURFACE_PILL_GRACE_MS - 1);
    expect(pill(SHELL_ENTRY_KIND)).not.toBe(null);
    await vi.advanceTimersByTimeAsync(1);
    expect(pill(SHELL_ENTRY_KIND)).toBe(null);
    expect(document.querySelector(".surface-viewer")).toBe(null);
    surfaces.dispose();
  });

  it("closes the viewer of a kind the grace stopped showing", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(SHELL_ENTRY_KIND);
    surfaces.set(finishedShells());
    await pressPill(CHECKLIST_ENTRY_KIND);

    await vi.advanceTimersByTimeAsync(SURFACE_PILL_GRACE_MS);

    expect(pill(SHELL_ENTRY_KIND)).toBe(null);
    expect(document.querySelector(".surface-checklist")).not.toBe(null);
    surfaces.dispose();
  });

  it("keeps the pill of a kind still running, and arms no timer for it", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    expect(vi.getTimerCount()).toBe(0);

    await vi.advanceTimersByTimeAsync(SURFACE_PILL_GRACE_MS * 3);
    expect(pill(SHELL_ENTRY_KIND)).not.toBe(null);
    surfaces.dispose();
  });

  it("arms one timer at a time and clears it on dispose", async () => {
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
  it("names the card with only its kind and keeps the history control beside close", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(AGENT_ENTRY_KIND);

    const head = document.querySelector(".surface-popover-head");
    expect(head.querySelector("strong").textContent).toBe("Agents");
    expect(head.textContent).not.toContain("Activity");
    expect([...head.querySelectorAll("button")].map((button) => button.className)).toEqual([
      "surface-history-toggle",
      "surface-popover-close",
    ]);
    expect(head.querySelector(".surface-popover-close").getAttribute("aria-label")).toBe("Close Agents");
    surfaces.dispose();
  });

  it("keeps the rows, the draft and the focus across a set that changes nothing", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(AGENT_ENTRY_KIND);
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

  it("stacks a section per phase, opening the running one and leaving the rest shut", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(WORKFLOW_ENTRY_KIND);

    expect(phaseSections().map((section) => section.dataset.state)).toEqual(["running", "pending"]);
    expect(phaseSections().map((section) => section.open)).toEqual([true, false]);
    expect(phaseSections().map(agentLabelsIn)).toEqual([["reader"], ["judge"]]);
    surfaces.dispose();
  });

  it("keeps the agent rows and the scroll when a workflow phase gains an agent", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(WORKFLOW_ENTRY_KIND);
    const viewerRegion = document.querySelector(".rail-surfaces-viewer");
    viewerRegion.scrollTop = 40;
    const [firstAgentRow] = agentRowsIn(phaseSections()[0]);
    expect(firstAgentRow).not.toBe(undefined);

    const grown = snapshot();
    grown.workflows[0].phases[0].agents.push({ id: "a3", label: "second reader", state: "queued" });
    surfaces.set(grown);

    const rowsAfter = agentRowsIn(phaseSections()[0]);
    expect(rowsAfter).toHaveLength(2);
    expect(rowsAfter[0]).toBe(firstAgentRow);
    expect(viewerRegion.scrollTop).toBe(40);
    surfaces.dispose();
  });

  it("keeps the agent rows and the scroll when the workflow's own head moves", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(WORKFLOW_ENTRY_KIND);
    const viewerRegion = document.querySelector(".rail-surfaces-viewer");
    viewerRegion.scrollTop = 40;
    const [firstAgentRow] = agentRowsIn(phaseSections()[0]);
    const [firstPhase] = phaseSections();

    const finished = snapshot();
    finished.workflows[0].state = "done";
    finished.workflows[0].description = "Read every parser and judge it";
    surfaces.set(finished);

    expect(agentRowsIn(phaseSections()[0])[0]).toBe(firstAgentRow);
    expect(phaseSections()[0]).toBe(firstPhase);
    expect(viewerRegion.scrollTop).toBe(40);
    expect(document.querySelector(".surface-workflow-head").textContent).toContain("Read every parser");
    surfaces.dispose();
  });

  it("leaves a phase the reader folded shut across the paints that follow", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(WORKFLOW_ENTRY_KIND);
    const [running] = phaseSections();
    running.open = false;

    const moved = snapshot();
    moved.workflows[0].phases[1].agents.push({ id: "a4", label: "second judge", state: "queued" });
    surfaces.set(moved);

    expect(phaseSections()[0]).toBe(running);
    expect(running.open).toBe(false);
    surfaces.dispose();
  });

  it("shows one workflow at a time, and offers no chooser while there is only one", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(WORKFLOW_ENTRY_KIND);

    expect(standing(".surface-workflow-choice")).toHaveLength(0);
    expect(document.querySelector(".surface-workflow-head").textContent).toContain("Review sweep");
    surfaces.dispose();
  });

  it("lets the reader reach the second workflow the pill counted", async () => {
    const surfaces = mount();
    const both = snapshot();
    both.workflows.push({
      id: "wf-2",
      name: "Fixture sweep",
      state: "running",
      phases: [{ title: "Write", agents: [{ id: "b1", label: "fixture writer", state: "running" }] }],
    });
    surfaces.set(both);
    await pressPill(WORKFLOW_ENTRY_KIND);

    expect(pillCount(WORKFLOW_ENTRY_KIND)).toBe("2");
    expect(pillCount(CHECKLIST_ENTRY_KIND)).toBe("1");
    const choices = [...document.querySelectorAll(".surface-workflow-choice")];
    expect(choices).toHaveLength(2);
    expect(choices[0].getAttribute("aria-pressed")).toBe("true");

    choices[1].click();

    expect(choices[1].getAttribute("aria-pressed")).toBe("true");
    expect(choices[0].getAttribute("aria-pressed")).toBe("false");
    expect(document.querySelector(".surface-workflow-head").textContent).toContain("Fixture sweep");
    expect(phaseSections().map(agentLabelsIn)).toEqual([["fixture writer"]]);
    surfaces.dispose();
  });

  it("falls back to the first workflow when the chosen one leaves the snapshot", async () => {
    const surfaces = mount();
    const both = snapshot();
    both.workflows.push({ id: "wf-2", name: "Fixture sweep", state: "running", phases: [] });
    surfaces.set(both);
    await pressPill(WORKFLOW_ENTRY_KIND);
    [...document.querySelectorAll(".surface-workflow-choice")][1].click();

    surfaces.set(snapshot());

    expect(document.querySelector(".surface-workflow-head").textContent).toContain("Review sweep");
    expect(standing(".surface-workflow-choice")).toHaveLength(0);
    surfaces.dispose();
  });

  it("paints entries with missing or repeated ids rather than throwing", async () => {
    const surfaces = mount();
    surfaces.set({
      shells: [{ description: "first", state: "running" }, { description: "second", state: "running" }],
      subagents: [
        { id: "same", label: "one", state: "running" },
        { id: "same", label: "two", state: "running" },
      ],
    });

    await pressPill(SHELL_ENTRY_KIND);
    expect(runningRows().map((row) => row.dataset.key)).toEqual(["shells-0", "shells-1"]);

    await pressPill(AGENT_ENTRY_KIND);
    expect(runningRows().map((row) => row.dataset.key)).toEqual(["same", "agent-1"]);
    surfaces.dispose();
  });

  it("keeps a fold the reader opened across a repaint", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(SHELL_ENTRY_KIND);
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

describe("the rows a viewer paints", () => {
  it("draws no menu on any row, and none on the workflow head", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());

    for (const kind of [AGENT_ENTRY_KIND, SHELL_ENTRY_KIND, CHECKLIST_ENTRY_KIND, WORKFLOW_ENTRY_KIND]) {
      await pressPill(kind);
      expect(document.querySelectorAll(".splitbtn")).toHaveLength(0);
      expect(document.querySelectorAll(".splitmenu")).toHaveLength(0);
    }
    surfaces.dispose();
  });

  it("never reaches the bridge itself, and offers no menu to reach it with", async () => {
    const source = coreSourceOf("agentSurfaces.js");

    expect(source).not.toContain("App.call");
    expect(source).not.toContain("../app.js");
    expect(source).not.toContain("mountSplitMenu");
    expect(source).not.toContain("onSendMessage");
  });

  it("redraws a shell that finished as a row inside the fold", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(SHELL_ENTRY_KIND);
    const [wasRunning] = runningRows();

    const finished = snapshot();
    finished.shells[0] = { ...finished.shells[0], state: "done", exit_code: 0 };
    surfaces.set(finished);

    expect(runningRows()).toEqual([]);
    const [row] = completedRows();
    expect(row).not.toBe(wasRunning);
    expect(row.textContent).toContain("cargo test");
    surfaces.dispose();
  });

  it("keeps the row a subagent's model gave a new trailing slot", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(AGENT_ENTRY_KIND);
    const [row] = completedRows();

    const named = snapshot();
    named.subagents[0] = { ...named.subagents[0], model: "haiku" };
    surfaces.set(named);

    expect(completedRows()[0]).toBe(row);
    expect(row.textContent).toContain("haiku");
    surfaces.dispose();
  });
});

describe("a set that would change nothing", () => {
  it("paints nothing at all", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(SHELL_ENTRY_KIND);
    const [row] = runningRows();
    const marker = document.createElement("span");
    marker.className = "paint-witness";
    row.appendChild(marker);

    surfaces.set(snapshot());

    expect(row.querySelector(".paint-witness")).toBe(marker);
    surfaces.dispose();
  });
});

describe("the control a subagent row jumps from", () => {
  it("opens the thread item the row was spawned by, and only for a row that names one", async () => {
    const onOpenThreadItem = vi.fn();
    const surfaces = mount({ onOpenThreadItem });
    surfaces.set(snapshot());
    await pressPill(AGENT_ENTRY_KIND);
    const [spawned] = completedRows();
    const [unspawned] = runningRows();

    spawned.querySelector("[data-call-sequence]").click();
    expect(onOpenThreadItem.mock.calls).toEqual([[SPAWNING_CALL_SEQUENCE]]);

    expect(unspawned.querySelector("[data-call-sequence]")).toBe(null);
    unspawned.click();
    expect(onOpenThreadItem).toHaveBeenCalledTimes(1);
    surfaces.dispose();
  });
});

describe("the fold the finished rows sit under", () => {
  it("holds every finished row behind the header history control, leaving no footer label", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(AGENT_ENTRY_KIND);

    expect(runningRows().map((row) => row.dataset.key)).toEqual(["s2"]);
    expect(completedRows().map((row) => row.dataset.key)).toEqual(["s1"]);
    expect(completedFold().querySelector(".surface-completed-head").hidden).toBe(true);
    expect(historyToggle().querySelector(".surface-history-count").textContent).toBe("1");
    expect(historyToggle().getAttribute("aria-label")).toBe("Show completed history (1)");
    expect(historyToggle().title).toBe("Show completed history (1)");
    expect(historyToggle().getAttribute("aria-pressed")).toBe("false");

    historyToggle().click();
    expect(completedFold().open).toBe(true);
    expect(historyToggle().getAttribute("aria-pressed")).toBe("true");
    expect(historyToggle().getAttribute("aria-label")).toBe("Hide completed history (1)");
    surfaces.dispose();
  });

  it("is not there at all while nothing has finished", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(SHELL_ENTRY_KIND);

    expect(runningRows()).toHaveLength(1);
    expect(completedFold()).toBe(null);
    expect(historyToggle().hidden).toBe(true);
    surfaces.dispose();
  });

  it("appears when the first row finishes and goes when the last finished row leaves", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(SHELL_ENTRY_KIND);

    const finished = snapshot();
    finished.shells[0] = { ...finished.shells[0], state: "done" };
    surfaces.set(finished);
    expect(completedFold()).not.toBe(null);
    expect(completedRows().map((row) => row.dataset.key)).toEqual(["sh1"]);
    expect(historyToggle().hidden).toBe(false);

    surfaces.set(snapshot());
    expect(completedFold()).toBe(null);
    expect(runningRows().map((row) => row.dataset.key)).toEqual(["sh1"]);
    expect(historyToggle().hidden).toBe(true);
    surfaces.dispose();
  });

  it("keeps the fold the reader opened, and the rows under it, across a repaint", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(AGENT_ENTRY_KIND);
    historyToggle().click();
    const fold = completedFold();
    const [finishedRow] = completedRows();

    const moved = snapshot();
    moved.subagents[1] = { ...moved.subagents[1], model: "haiku" };
    surfaces.set(moved);

    expect(completedFold()).toBe(fold);
    expect(fold.open).toBe(true);
    expect(historyToggle().getAttribute("aria-pressed")).toBe("true");
    expect(completedRows()[0]).toBe(finishedRow);
    surfaces.dispose();
  });

  it("opens history when an expanded running agent completes, keeping its detail open", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(AGENT_ENTRY_KIND);
    const running = document.querySelector('.surface-running > [data-key="s2"]');
    running.querySelector(".surface-agent-summary").click();
    await new Promise((resolve) => setTimeout(resolve, 0));

    const finished = snapshot();
    finished.subagents[1] = { ...finished.subagents[1], state: "done", result: "Review complete" };
    surfaces.set(finished);

    const completed = document.querySelector('.surface-completed-rows > [data-key="s2"]');
    expect(completed.open).toBe(true);
    expect(completedFold().open).toBe(true);
    expect(historyToggle().getAttribute("aria-pressed")).toBe("true");
    expect(historyToggle().getAttribute("aria-label")).toBe("Hide completed history (2)");
    surfaces.dispose();
  });

  it("counts the fold again when another row finishes", async () => {
    const surfaces = mount();
    surfaces.set(snapshot());
    await pressPill(AGENT_ENTRY_KIND);

    const both = snapshot();
    both.subagents[1] = { ...both.subagents[1], state: "done" };
    surfaces.set(both);

    expect(historyToggle().querySelector(".surface-history-count").textContent).toBe("2");
    expect(historyToggle().getAttribute("aria-label")).toBe("Show completed history (2)");
    expect(completedRows().map((row) => row.dataset.key)).toEqual(["s1", "s2"]);
    expect(runningRows()).toEqual([]);
    surfaces.dispose();
  });
});
