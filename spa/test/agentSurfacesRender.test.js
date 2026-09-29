// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { coreSourceOf } from "./coreSource.js";
import { recordedSurfaces } from "./recordedSurfaces.js";
import {
  CLIP_SELECTOR,
  PRESSABLE_CLIP_SELECTOR,
  COMPLETED_FOLD_HEAD_SELECTOR,
  COMPLETED_FOLD_SELECTOR,
  SURFACE_SELECTOR,
  agentRowHtml,
  checklistItemHtml,
  clippedTextHtml,
  completedFoldHtml,
  kindViewerHtml,
  phaseSectionHtml,
  runningAndCompletedViewerHtml,
  shellRowHtml,
  surfacePillHtml,
  workflowChoiceHtml,
  workflowViewerHtml,
} from "../src/core/agentSurfacesRender.js";
import {
  AGENT_ENTRY_KIND,
  CHECKLIST_ENTRY_KIND,
  SHELL_ENTRY_KIND,
  SURFACE_KINDS,
  WORKFLOW_ENTRY_KIND,
  agentRows,
  rowSubject,
  runningAndCompletedRows,
  surfacePills,
  surfaceRows,
  surfaceStateMark,
  workflowChoicesWorthOffering,
  workflowPhases,
} from "../src/core/agentSurfacesModel.js";
import { patchList } from "../src/core/patchList.js";
import { KEYED_LIST_ATTRIBUTE } from "../src/core/domPatch.js";

const HOSTILE_MARKUP = "</div><img onerror=x>";

const parseHtml = (html) => {
  const holder = globalThis.document.createElement("div");
  holder.innerHTML = html;
  return holder;
};

const expectEscaped = (html) => {
  expect(html).not.toContain("<img");
  expect(html).toContain("&lt;/div&gt;");
};

const factValue = (row, label) =>
  [...row.querySelectorAll(".surface-agent-fact")]
    .find((fact) => fact.querySelector("dt").textContent === label)
    .querySelector("dd").textContent;

const readerEntry = {
  id: "a1",
  label: "Reader",
  model: "haiku",
  state: "running",
  duration_ms: 65000,
  tokens: 1200,
  tool_calls: 4,
  last_tool: { name: "Read", summary: "bridge/src/app.rs" },
};

const freshWorkflow = {
  id: "w1",
  name: "Review",
  description: "Read the branch and report",
  state: "running",
  phases: [
    { title: "Read", agents: [{ label: "Reader", state: "queued", model: "haiku" }, { label: "Counter", state: "queued", model: "haiku" }] },
    { title: "Write", agents: [] },
  ],
};

const STARTED_AT = 1788291725678;
const stagedWorkflow = {
  id: "w2",
  name: "Sweep",
  state: "running",
  phases: [
    {
      title: "Read",
      agents: [
        { id: "a1", label: "Reader", state: "done", started_at: STARTED_AT, duration_ms: 20000 },
        { id: "a2", label: "Skimmer", state: "running", started_at: STARTED_AT + 5000 },
      ],
    },
    { title: "Write", agents: [{ id: "a3", label: "Writer", state: "queued" }] },
  ],
};

const oneWorkflow = (workflow) => ({ workflows: [workflow] });
const phasesOf = (workflow, nowMs = 0) => workflowPhases(oneWorkflow(workflow), 0, { nowMs });
const workflowRow = (workflow) => surfaceRows("workflows", oneWorkflow(workflow))[0];
const subagentViewerHtml = (rows) =>
  runningAndCompletedViewerHtml(AGENT_ENTRY_KIND, runningAndCompletedRows(rows), agentRowHtml);
const shellViewerHtml = (rows) =>
  runningAndCompletedViewerHtml(SHELL_ENTRY_KIND, runningAndCompletedRows(rows), shellRowHtml);
const checklistViewerHtml = (rows) => kindViewerHtml(CHECKLIST_ENTRY_KIND, rows, checklistItemHtml);

describe("agentRowHtml", () => {
  it("is one renderer: a workflow agent and a subagent of the same entry paint the same bytes", () => {
    const fromWorkflow = phasesOf({ phases: [{ title: "Read", agents: [readerEntry] }] })[0].rows[0];
    const fromSubagents = surfaceRows("subagents", { subagents: [readerEntry] })[0];
    expect(agentRowHtml(fromWorkflow)).toBe(agentRowHtml(fromSubagents));
    expect(agentRowHtml(fromWorkflow)).toBe(agentRowHtml(agentRows([readerEntry])[0]));
  });

  it("draws a compact row as a native disclosure with all reported details behind its summary", () => {
    const row = parseHtml(agentRowHtml(agentRows([readerEntry])[0], { compact: true })).firstElementChild;
    expect(row.tagName).toBe("DETAILS");
    expect(row.open).toBe(false);
    expect(row.firstElementChild.tagName).toBe("SUMMARY");
    expect([...row.children].map((line) => line.className)).toEqual([
      "surface-row-head surface-agent-summary",
      "surface-agent-details",
    ]);
    expect(row.querySelector(".surface-row-label").textContent).toBe("Reader");
    expect(row.querySelector(".surface-agent-summary > .surface-row-model").textContent).toBe("haiku");
    expect(factValue(row, "Model")).toBe("haiku");
    expect(factValue(row, "Current activity")).toBe("Read bridge/src/app.rs");
    expect(factValue(row, "Tokens")).toBe("1200");
    expect(factValue(row, "Tool calls")).toBe("4");
  });

  it("omits optional facts that were not reported without guessing inherited defaults", () => {
    const row = parseHtml(agentRowHtml(agentRows([readerEntry])[0])).firstElementChild;
    expect([...row.querySelectorAll("dt")].map((term) => term.textContent)).toEqual([
      "State",
      "Model",
      "Current activity",
      "Tokens",
      "Tool calls",
    ]);
    expect(factValue(row, "State")).toBe("Running");
  });

  it("places the reported model immediately before the ticker in the agent summary", () => {
    const row = parseHtml(
      agentRowHtml(agentRows([{ ...readerEntry, started_at: STARTED_AT }], { nowMs: STARTED_AT + 65000 })[0]),
    ).firstElementChild;
    const summary = row.querySelector(".surface-agent-summary");

    expect([...summary.children].slice(1).map((child) => child.className)).toEqual([
      "surface-clip surface-row-label",
      // Never clipped (#257): the short name fits whole.
      "surface-row-model surface-row-head-model",
      "surface-row-clock",
    ]);
    expect(summary.querySelector(".surface-row-head-model").title).toBe("haiku");
  });

  it("does not reserve summary space when an agent reports no model", () => {
    const row = parseHtml(agentRowHtml(agentRows([{ id: "a1", label: "Reader", state: "running" }])[0]));
    expect(row.querySelector(".surface-agent-summary > .surface-row-model")).toBe(null);
  });

  it("leaves the details list empty when every optional fact is unavailable or blank", () => {
    const row = parseHtml(
      agentRowHtml(
        agentRows([{ id: "a1", description: "  ", model: "", reasoning_effort: "\t", state: "" }])[0],
      ),
    ).firstElementChild;

    expect(row.querySelectorAll(".surface-agent-fact")).toHaveLength(0);
    expect(row.textContent).not.toContain("Not reported");
  });

  it("keeps reported zero counters visible", () => {
    const row = parseHtml(
      agentRowHtml(agentRows([{ id: "a1", label: "Reader", state: "running", tokens: 0, tool_calls: 0 }])[0]),
    ).firstElementChild;

    expect(factValue(row, "Tokens")).toBe("0");
    expect(factValue(row, "Tool calls")).toBe("0");
  });

  it("prints the model name the row arrived with", () => {
    const named = agentRows([readerEntry], { modelLabel: (id) => `Local ${id}` })[0];
    expect(parseHtml(agentRowHtml(named)).querySelector(".surface-row-model").textContent).toBe("Local haiku");
  });

  // #257: the summary wears the short name; its title and the details keep
  // the full label, where there is room for it.
  it("shortens the model in the summary and names it whole in its title and details", () => {
    const named = agentRows([{ ...readerEntry, model: "claude-haiku-4-5-20251001" }], { modelLabel: () => "Claude Haiku 4.5" })[0];
    const row = parseHtml(agentRowHtml(named)).firstElementChild;
    const summaryModel = row.querySelector(".surface-agent-summary > .surface-row-model");
    expect([summaryModel.textContent, summaryModel.title]).toEqual(["Haiku 4.5", "Claude Haiku 4.5"]);
    expect(factValue(row, "Model")).toBe("Claude Haiku 4.5");
  });

  it("keeps the delegated task and reasoning effort visible beside the canonical agent path", () => {
    const [agent] = agentRows([
      {
        id: "thread-1",
        label: "/root/tool_display",
        description: "Explain tool activity in the timeline",
        model: "gpt-5.6-sol",
        reasoning_effort: "high",
        state: "running",
      },
    ]);
    const row = parseHtml(agentRowHtml(agent)).firstElementChild;

    expect(row.querySelector(".surface-row-label").textContent).toBe("/root/tool_display");
    expect(factValue(row, "Description")).toBe("Explain tool activity in the timeline");
    expect(row.querySelector(".surface-row-model").textContent).toBe("5.6 Sol");
    expect(row.querySelector(".surface-row-effort").textContent).toBe("high");
    expect(factValue(row, "State")).toBe("Running");
  });

  it("draws the state mark the model named and nothing for a state it does not recognise", () => {
    const marked = parseHtml(agentRowHtml(agentRows([readerEntry])[0]));
    expect(marked.querySelectorAll("[data-outcome]").length).toBe(1);
    const unknown = parseHtml(agentRowHtml(agentRows([{ id: "a1", label: "Reader", state: "banana" }])[0]));
    expect(unknown.querySelectorAll("[data-outcome]").length).toBe(0);
  });

  it("draws the result of a finished agent and the error of a failed one, on the one line", () => {
    const done = parseHtml(agentRowHtml(agentRows([{ id: "a1", label: "Reader", state: "done", result: "42 files" }])[0]));
    expect(done.querySelector(".surface-row-detail").textContent).toBe("42 files");
    const failed = parseHtml(
      agentRowHtml(agentRows([{ id: "a1", label: "Reader", state: "failed", error: "no such path" }])[0]),
    );
    expect(failed.querySelector(".surface-row-detail").textContent).toBe("no such path");
    expect(failed.querySelector(".surface-row-error")).toBeTruthy();
  });

  it("gives a running row a clock span to tick in, and every other row none", () => {
    const running = agentRows([{ ...readerEntry, started_at: STARTED_AT }], { nowMs: STARTED_AT + 65000 })[0];
    const clock = parseHtml(agentRowHtml(running)).querySelector(".surface-row-clock");
    expect(clock.dataset.runningSince).toBe(String(STARTED_AT));
    expect(clock.textContent).toBe("1:05");

    expect(agentRowHtml(agentRows([{ id: "a1", label: "Reader", state: "queued" }])[0])).not.toContain(
      "surface-row-clock",
    );
  });

  it("freezes a finished agent's clock in the slot it ticked in, reading as a phase clock reads", () => {
    const finished = parseHtml(agentRowHtml(agentRows([{ ...readerEntry, state: "done" }])[0]));
    const clock = finished.querySelector(".surface-row-clock");
    expect(clock.textContent).toBe("1:05");
    expect(clock.hasAttribute("data-running-since")).toBe(false);
  });

  it("gives a running shell the same clock span through the same head", () => {
    const [row] = surfaceRows(SHELL_ENTRY_KIND, {
      shells: [{ id: "s1", description: "cargo test", state: "running", started_at: 1788291725678 }],
    }, { nowMs: 1788291725678 });
    expect(parseHtml(shellRowHtml(row)).querySelector(".surface-row-clock").dataset.runningSince).toBe(
      "1788291725678",
    );
  });

  it("draws no menu of any kind on a row", () => {
    const row = parseHtml(agentRowHtml(agentRows([readerEntry])[0]));
    expect(row.querySelector(".splitbtn")).toBe(null);
    expect(row.querySelector(".splitmenu")).toBe(null);
  });

  it("hangs the jump to the spawning call on one control of its own, never on the whole row", () => {
    const spawned = parseHtml(agentRowHtml(agentRows([{ ...readerEntry, call_sequence: 12 }])[0]));
    const jump = spawned.querySelector("[data-call-sequence]");
    expect(jump.tagName).toBe("BUTTON");
    expect(jump.dataset.callSequence).toBe("12");
    expect(jump.getAttribute("aria-label")).toBe(jump.getAttribute("title"));
    expect(spawned.firstElementChild.hasAttribute("data-call-sequence")).toBe(false);
    expect(agentRowHtml(agentRows([readerEntry])[0])).not.toContain("data-call-sequence");
  });

  it("escapes everything the model can put in it", () => {
    const html = agentRowHtml(
      agentRows([
        {
          id: HOSTILE_MARKUP,
          label: HOSTILE_MARKUP,
          model: HOSTILE_MARKUP,
          state: "running",
          result: HOSTILE_MARKUP,
          last_tool: { name: HOSTILE_MARKUP, summary: HOSTILE_MARKUP },
        },
      ])[0],
    );
    expectEscaped(html);
  });
});

describe("one row renderer per kind, exported for the keyed paint", () => {
  const shellRow = surfaceRows("shells", {
    shells: [{ id: "s1", description: "npm test", state: "running", tail: ["one"] }],
  })[0];
  const checklistRow = surfaceRows("checklist", { checklist: [{ id: "t1", subject: "one", state: "pending" }] })[0];
  const phaseRow = phasesOf(freshWorkflow)[0];

  it("gives back exactly one element per row, which is what patchList renders with", () => {
    const markupOfEveryKind = [
      agentRowHtml(agentRows([readerEntry])[0]),
      phaseSectionHtml(phaseRow),
      shellRowHtml(shellRow),
      checklistItemHtml(checklistRow),
    ];
    for (const markup of markupOfEveryKind) expect(parseHtml(markup).children.length).toBe(1);
  });

  it("is the one source of each kind's row markup, its viewer being the frame around it", () => {
    expect(shellViewerHtml([shellRow])).toContain(shellRowHtml(shellRow));
    expect(checklistViewerHtml([checklistRow])).toContain(checklistItemHtml(checklistRow));
    const phases = phasesOf(freshWorkflow);
    expect(workflowViewerHtml(workflowRow(freshWorkflow), [], phases)).toContain(phaseSectionHtml(phases[0]));
  });

  it("leaves each viewer a frame the mount can fill when it is handed no rows", () => {
    for (const empty of [shellViewerHtml([]), checklistViewerHtml([]), subagentViewerHtml([])]) {
      expect(parseHtml(empty).children.length).toBe(1);
      expect(parseHtml(empty).querySelector(".surface-viewer")).toBeTruthy();
    }
  });

  it("paints the name the row goes by, for a row carrying only an id", () => {
    const namelessRows = [
      [AGENT_ENTRY_KIND, agentRowHtml, surfaceRows(AGENT_ENTRY_KIND, { subagents: [{ id: "a1", state: "running" }] })[0]],
      [SHELL_ENTRY_KIND, shellRowHtml, surfaceRows(SHELL_ENTRY_KIND, { shells: [{ id: "s1", state: "running" }] })[0]],
      [
        CHECKLIST_ENTRY_KIND,
        checklistItemHtml,
        surfaceRows(CHECKLIST_ENTRY_KIND, { checklist: [{ id: "t1", state: "pending" }] })[0],
      ],
    ];
    for (const [kind, renderRow, row] of namelessRows) {
      const painted = parseHtml(renderRow(row)).querySelector(".surface-row-label").textContent;
      expect(painted).toBe(rowSubject(kind, row));
      expect(painted.length).toBeGreaterThan(0);
    }
  });

  it("stamps each row with the key it arrived under, so a later keyed paint keeps it", () => {
    const rows = surfaceRows(CHECKLIST_ENTRY_KIND, {
      checklist: [{ id: "t1", subject: "one", state: "pending" }, { id: "t2", subject: "two", state: "completed" }],
    });
    const viewer = parseHtml(checklistViewerHtml(rows)).querySelector(".surface-viewer");
    expect([...viewer.children].map((child) => child.dataset.key)).toEqual(rows.map((row) => row.key));
    const painted = [...viewer.children];
    patchList(viewer, rows, { keyOf: (row) => row.key, render: checklistItemHtml });
    expect([...viewer.children]).toEqual(painted);
  });
});

describe("surfacePillHtml", () => {
  const [busyPill, settledPill] = surfacePills({
    shells: [{ id: "s1", state: "running" }],
    checklist: [{ id: "t1", state: "completed" }],
  });
  const rendered = (pill, openKind) => parseHtml(surfacePillHtml(pill, openKind)).firstElementChild;

  it("presses exactly the open kind and no other", () => {
    expect(rendered(busyPill, "shells").getAttribute("aria-pressed")).toBe("true");
    expect(rendered(settledPill, "shells").getAttribute("aria-pressed")).toBe("false");
  });

  it("presses nothing when no pill is open", () => {
    expect(rendered(busyPill, null).getAttribute("aria-pressed")).toBe("false");
    expect(rendered(settledPill, null).getAttribute("aria-pressed")).toBe("false");
  });

  it("shows running counts for activity and completed progress for tasks", () => {
    const busy = rendered(busyPill, null);
    expect([...busy.children].map((child) => child.className)).toEqual([
      "surface-pill-label",
      "surface-pill-count",
    ]);
    expect(busy.querySelector(".surface-pill-count").textContent).toBe("1");

    const settled = rendered(settledPill, null);
    expect(settled.querySelector(".surface-pill-count").hidden).toBe(false);
    expect(settled.querySelector(".surface-pill-count").textContent).toBe("1/1");
    expect(settled.textContent).toContain("Checklist");
  });

  it("hands the cap and the pill itself to the motion primitive", () => {
    expect(rendered(busyPill, null).hasAttribute("data-motion")).toBe(true);
    expect(rendered(busyPill, null).querySelector(".surface-pill-count").hasAttribute("data-motion")).toBe(true);
  });

  it("renders a workflows pill whose only workflow has finished as the label alone", () => {
    const [finished] = surfacePills({ workflows: [{ id: "w1", name: "Review", state: "done" }] });
    const button = rendered(finished, null);
    expect(button.textContent.trim()).toBe("Workflows");
    expect(button.querySelector(".surface-pill-count").hidden).toBe(true);
  });

  it("wears no dot, the count being what says work is running", () => {
    expect(surfacePillHtml(busyPill, null)).not.toContain("sdot");
  });

  it("says the label and the count of the pill", () => {
    const busy = rendered(busyPill, "shells");
    expect(busy.textContent).toContain("Shells");
    expect(busy.textContent).toContain("1");
  });

  it("escapes a pill label", () => {
    expectEscaped(surfacePillHtml({ kind: "shells", label: HOSTILE_MARKUP, count: 1 }, null));
  });
});

describe("the workflow viewer, stacked", () => {
  const view = (workflow, choices = [], nowMs = 0) =>
    workflowViewerHtml(workflowRow(workflow), choices, phasesOf(workflow, nowMs));

  it("paints both of the id-less queued agents a fresh workflow carries, each marked queued", () => {
    const [phase] = phasesOf(freshWorkflow);
    const painted = parseHtml(phase.rows.map((row) => agentRowHtml(row)).join(""));
    const agentRowElements = [...painted.querySelectorAll(".surface-agent")];
    expect(agentRowElements.length).toBe(2);
    expect(painted.textContent).toContain("Reader");
    expect(painted.textContent).toContain("Counter");
    const queuedMark = surfaceStateMark(AGENT_ENTRY_KIND, "queued");
    expect(agentRowElements.map((row) => row.querySelector("[data-outcome]").dataset.outcome)).toEqual([
      queuedMark.mark,
      queuedMark.mark,
    ]);
  });

  it("stacks one section per phase, in order, each with an empty list for its agents", () => {
    const painted = parseHtml(view(stagedWorkflow));
    const sections = [...painted.querySelectorAll(".surface-phase")];
    expect(sections.map((section) => section.tagName)).toEqual(["DETAILS", "DETAILS"]);
    expect(sections.map((section) => section.dataset.key)).toEqual(["phase-0", "phase-1"]);
    expect(sections.map((section) => section.querySelector(".surface-phase-count").textContent)).toEqual([
      "1/2",
      "0/1",
    ]);
    expect(sections.map((section) => section.querySelector(".surface-phase-agents").innerHTML)).toEqual(["", ""]);
    expect(phasesOf(stagedWorkflow)[1].rows.map((row) => row.subject)).toEqual(["Writer"]);
  });

  it("names the workflow and marks its state on one line, with the description clipped beneath", () => {
    const head = parseHtml(view(freshWorkflow)).querySelector(".surface-workflow-head");
    expect(head.querySelector(".surface-row-label").textContent).toBe("Review");
    expect(head.querySelector("[data-outcome]")).toBeTruthy();
    expect(head.querySelector(".splitbtn")).toBe(null);
    const note = head.querySelector(".surface-row-note");
    expect(note.textContent).toBe("Read the branch and report");
    expect(note.getAttribute("data-clip-lines")).toBe("2");
  });

  it("names each workflow the reader may choose between, pressing the one on show", () => {
    const twoWorkflows = { workflows: [freshWorkflow, { id: "w2", name: "Fixtures", state: "running" }] };
    const choices = workflowChoicesWorthOffering(twoWorkflows, 1);
    const html = view(freshWorkflow, choices);
    const buttons = [...parseHtml(html).querySelectorAll(".surface-workflow-choice")];
    expect(buttons.map((button) => button.getAttribute("aria-pressed"))).toEqual(["false", "true"]);
    expect(buttons[1].textContent).toContain("Fixtures");
    expect(html).toContain(workflowChoiceHtml(choices[0]));
  });

  it("escapes the workflow's own name and its phase titles", () => {
    const poisoned = { id: "w1", name: HOSTILE_MARKUP, state: "running", phases: [{ title: HOSTILE_MARKUP, agents: [{ label: HOSTILE_MARKUP }] }] };
    const html = view(poisoned, workflowChoicesWorthOffering({ workflows: [poisoned, poisoned] }, 0));
    expectEscaped(html);
  });
});

describe("phaseSectionHtml", () => {
  const runningPhase = (nowMs = STARTED_AT + 65000) => phasesOf(stagedWorkflow, nowMs)[0];

  it("is one keyed details, its summary saying the title, the count and the phase clock", () => {
    const section = parseHtml(phaseSectionHtml(runningPhase())).firstElementChild;
    expect(section.tagName).toBe("DETAILS");
    expect(section.dataset.key).toBe("phase-0");
    expect(section.dataset.state).toBe("running");
    const summary = section.querySelector(".surface-phase-head");
    expect(summary.tagName).toBe("SUMMARY");
    expect(summary.querySelector(".surface-phase-title").textContent).toBe("Read");
    expect(summary.querySelector(".surface-phase-count").textContent).toBe("1/2");
    const clock = summary.querySelector(".surface-row-clock");
    expect(clock.dataset.runningSince).toBe(String(STARTED_AT));
    expect(clock.textContent).toBe("1:05");
  });

  it("says nothing about being open: which fold stands is the reader's and the mount's", () => {
    expect(parseHtml(phaseSectionHtml(runningPhase())).firstElementChild.hasAttribute("open")).toBe(false);
  });

  it("leaves its agents to the painter that keys them, holding an empty list for it", () => {
    const list = parseHtml(phaseSectionHtml(runningPhase())).querySelector(".surface-phase-agents");
    expect(list.hasAttribute(KEYED_LIST_ATTRIBUTE)).toBe(true);
    expect(list.innerHTML).toBe("");
    expect(phaseSectionHtml(runningPhase())).not.toContain("surface-agent");
  });

  it("carries no clock at all for a phase nothing has started", () => {
    const pending = phasesOf(stagedWorkflow)[1];
    expect(phaseSectionHtml(pending)).not.toContain("surface-row-clock");
  });

  it("escapes a phase title", () => {
    expectEscaped(phaseSectionHtml(phasesOf({ phases: [{ title: HOSTILE_MARKUP, agents: [] }] })[0]));
  });
});

describe("the text a viewer clips", () => {
  it("is one span, titled with the whole of it, saying how many lines it shows", () => {
    const clipped = parseHtml(clippedTextHtml("a long line", { className: "surface-row-note", lines: 2 }));
    const span = clipped.firstElementChild;
    expect(span.getAttribute("title")).toBe("a long line");
    expect(span.getAttribute("data-clip-lines")).toBe("2");
    expect(span.className).toBe("surface-clip surface-row-note");
    expect(span.textContent).toBe("a long line");
    expect(span.matches(CLIP_SELECTOR)).toBe(true);
  });

  it("is a press target the keyboard can reach, unless its caller says the clip is for hovering", () => {
    const pressable = parseHtml(clippedTextHtml("a long line")).firstElementChild;
    expect(pressable.getAttribute("role")).toBe("button");
    expect(pressable.getAttribute("tabindex")).toBe("0");
    expect(pressable.getAttribute("aria-expanded")).toBe("false");
    expect(pressable.matches(PRESSABLE_CLIP_SELECTOR)).toBe(true);

    const hovered = parseHtml(clippedTextHtml("a long line", { pressable: false })).firstElementChild;
    expect(hovered.hasAttribute("role")).toBe(false);
    expect(hovered.hasAttribute("tabindex")).toBe(false);
    expect(hovered.hasAttribute("aria-expanded")).toBe(false);
    expect(hovered.matches(PRESSABLE_CLIP_SELECTOR)).toBe(false);
  });

  it("leaves the press to the fold and to the button that hold these two clips", () => {
    const title = parseHtml(phaseSectionHtml(phasesOf(stagedWorkflow)[0])).querySelector(".surface-phase-title");
    expect(title.matches(CLIP_SELECTOR)).toBe(true);
    expect(title.matches(PRESSABLE_CLIP_SELECTOR)).toBe(false);

    const [choice] = workflowChoicesWorthOffering({ workflows: [freshWorkflow, freshWorkflow] }, 0);
    const name = parseHtml(workflowChoiceHtml(choice)).querySelector(".surface-choice-name");
    expect(name.matches(CLIP_SELECTOR)).toBe(true);
    expect(name.matches(PRESSABLE_CLIP_SELECTOR)).toBe(false);
    expect(name.textContent).toBe("Review");
  });

  it("shows one line unless it is asked for more, and escapes what it is given", () => {
    expect(parseHtml(clippedTextHtml("one")).firstElementChild.getAttribute("data-clip-lines")).toBe("1");
    expectEscaped(clippedTextHtml(HOSTILE_MARKUP));
  });

  it("clips the summary label but leaves expanded agent details readable in full", () => {
    const row = parseHtml(agentRowHtml(agentRows([{ ...readerEntry, result: "Read every file" }])[0], { compact: true }));
    expect(row.querySelector(".surface-row-label").matches(CLIP_SELECTOR)).toBe(true);
    expect(row.querySelector(".surface-row-label").matches(PRESSABLE_CLIP_SELECTOR)).toBe(false);
    expect(row.querySelector(".surface-row-detail").matches(CLIP_SELECTOR)).toBe(false);
    const note = parseHtml(
      checklistItemHtml(surfaceRows(CHECKLIST_ENTRY_KIND, { checklist: [{ id: "t1", subject: "one", description: "two", state: "pending" }] })[0]),
    ).querySelector(".surface-row-note");
    expect(note.matches(CLIP_SELECTOR)).toBe(true);
  });
});

describe("subagentViewerHtml", () => {
  it("draws the same rows through the same agent row renderer", () => {
    const rows = surfaceRows("subagents", { subagents: [readerEntry, { id: "a2", label: "Writer", state: "done" }] });
    const html = subagentViewerHtml(rows);
    expect(html).toContain(agentRowHtml(rows[0]));
    expect(html).toContain(agentRowHtml(rows[1]));
    expect(parseHtml(html).querySelectorAll(".surface-agent").length).toBe(2);
  });

  it("escapes a subagent label", () => {
    const html = subagentViewerHtml(surfaceRows("subagents", { subagents: [{ id: "a1", label: HOSTILE_MARKUP }] }));
    expectEscaped(html);
  });
});

describe("shellViewerHtml", () => {
  const rows = (shells) => surfaceRows("shells", { shells });

  it("folds each shell's tail into a pre inside a details", () => {
    const painted = parseHtml(
      shellViewerHtml(rows([{ id: "s1", description: "npm test", state: "running", tail: ["one", "two"] }])),
    );
    const fold = painted.querySelector("details");
    expect(fold).toBeTruthy();
    expect(fold.open).toBe(false);
    expect(fold.querySelector("pre").textContent).toContain("one");
    expect(fold.querySelector("pre").textContent).toContain("two");
  });

  it("escapes a tail line", () => {
    const html = shellViewerHtml(rows([{ id: "s1", description: "npm test", state: "running", tail: ["<script>alert(1)</script>"] }]));
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
  });

  it("says each shell's description, its state mark and its exit code", () => {
    const painted = parseHtml(
      shellViewerHtml(rows([{ id: "s1", description: "npm test", state: "done", exit_code: 1, tail: ["boom"] }])),
    );
    expect(painted.textContent).toContain("npm test");
    expect(painted.textContent).toContain("1");
    expect(painted.querySelectorAll("[data-outcome]").length).toBe(1);
  });

  it("offers no fold onto a shell that has printed nothing", () => {
    const painted = parseHtml(shellViewerHtml(rows([{ id: "s1", description: "npm test", state: "running" }])));
    expect(painted.querySelector("details")).toBe(null);
  });

  it("escapes a shell description", () => {
    const html = shellViewerHtml(rows([{ id: "s1", description: HOSTILE_MARKUP, state: "running" }]));
    expectEscaped(html);
  });
});

describe("checklistViewerHtml", () => {
  const rows = (checklist) => surfaceRows("checklist", { checklist });

  it("marks every state it recognises", () => {
    const painted = parseHtml(
      checklistViewerHtml(
        rows([
          { id: "t1", subject: "one", state: "pending" },
          { id: "t2", subject: "two", state: "in_progress" },
          { id: "t3", subject: "three", state: "completed" },
          { id: "t4", subject: "four", state: "blocked" },
        ]),
      ),
    );
    expect(painted.querySelectorAll(".surface-checklist-item").length).toBe(4);
    expect(painted.querySelectorAll("[data-outcome]").length).toBe(4);
    expect(painted.querySelector("[data-key='t3']").classList.contains("is-completed")).toBe(true);
    expect(painted.querySelector("[data-key='t2']").classList.contains("is-completed")).toBe(false);
  });

  it("marks nothing for a state it does not recognise", () => {
    const painted = parseHtml(checklistViewerHtml(rows([{ id: "t1", subject: "one", state: "banana" }])));
    expect(painted.querySelectorAll(".surface-checklist-item").length).toBe(1);
    expect(painted.querySelectorAll("[data-outcome]").length).toBe(0);
  });

  it("says each item's subject", () => {
    expect(parseHtml(checklistViewerHtml(rows([{ id: "t1", subject: "Read the spec", state: "pending" }]))).textContent).toContain(
      "Read the spec",
    );
  });

  it("says an item's description beside its subject", () => {
    const painted = parseHtml(
      checklistViewerHtml(
        rows([{ id: "t1", subject: "Land the fold", description: "and pin it to the fixture", state: "pending" }]),
      ),
    );
    expect(painted.querySelector(".surface-row-note").textContent).toBe("and pin it to the fixture");
  });

  it("says a description that only repeats the subject once", () => {
    const painted = parseHtml(
      checklistViewerHtml(rows([{ id: "t1", subject: "Land the fold", description: "Land the fold", state: "pending" }])),
    );
    expect(painted.querySelector(".surface-row-note")).toBe(null);
    expect(painted.textContent.match(/Land the fold/g)).toHaveLength(1);
  });

  it("escapes an item's subject and its description", () => {
    expectEscaped(checklistViewerHtml(rows([{ id: "t1", subject: HOSTILE_MARKUP, state: "pending" }])));
    expectEscaped(
      checklistViewerHtml(rows([{ id: "t1", subject: "one", description: HOSTILE_MARKUP, state: "pending" }])),
    );
  });
});

describe("the wire the bridge actually builds", () => {
  const recorded = recordedSurfaces();

  it("paints the recorded subagent's label, model-less head, stats and result", () => {
    const [row] = surfaceRows(AGENT_ENTRY_KIND, recorded);
    const painted = parseHtml(agentRowHtml(row));
    expect(painted.querySelector(".surface-row-label").textContent).toBe(recorded.subagents[0].label);
    expect(painted.querySelector("[data-call-sequence]").dataset.callSequence).toBe(
      String(recorded.subagents[0].call_sequence),
    );
    expect(factValue(painted, "Tokens")).toBe(String(recorded.subagents[0].tokens));
    expect(factValue(painted, "Tool calls")).toBe(String(recorded.subagents[0].tool_calls));
    expect(painted.querySelector(".surface-row-detail").textContent).toBe(recorded.subagents[0].result);
  });

  it("paints the recorded shell's exit code and the recorded checklist's notes", () => {
    const painted = parseHtml(shellViewerHtml(surfaceRows(SHELL_ENTRY_KIND, recorded)));
    expect(painted.textContent).toContain("exit 0");
    expect(painted.querySelectorAll("[data-outcome]").length).toBe(1);
    const items = parseHtml(checklistViewerHtml(surfaceRows(CHECKLIST_ENTRY_KIND, recorded)));
    expect([...items.querySelectorAll(".surface-row-note")].map((note) => note.textContent)).toEqual(
      recorded.checklist.map((item) => item.description),
    );
  });

  it("paints the recorded workflow's head, phases and agents", () => {
    const phases = workflowPhases(recorded);
    const painted = parseHtml(workflowViewerHtml(surfaceRows(WORKFLOW_ENTRY_KIND, recorded)[0], [], phases));
    expect(painted.querySelector(".surface-workflow-head").textContent).toContain("readme-analysis");
    expect([...painted.querySelectorAll(".surface-phase-count")].map((count) => count.textContent)).toEqual([
      "2/2",
      "1/1",
    ]);
    expect(phases.map((phase) => phase.rows.map((row) => row.subject))).toEqual([
      ["line-counter", "char-counter"],
      ["summarizer"],
    ]);
  });
});

describe("the renderer is pure markup", () => {
  it("queries no page, wires no handler and calls no bridge", () => {
    const source = coreSourceOf("agentSurfacesRender.js");
    for (const forbidden of ["document", "window", "App", "localStorage"]) {
      expect(source).not.toContain(forbidden);
    }
  });

  it("names no surface kind of its own, taking every one it keys by from the model", () => {
    const source = coreSourceOf("agentSurfacesRender.js");
    for (const kind of SURFACE_KINDS) expect(source).not.toContain(`"${kind}"`);
    for (const constantName of ["AGENT_ENTRY_KIND", "SHELL_ENTRY_KIND", "CHECKLIST_ENTRY_KIND"]) {
      expect(source).toContain(constantName);
    }
  });

  it("reads a row's subject off the row rather than asking the model a second time", () => {
    const source = coreSourceOf("agentSurfacesRender.js");
    expect(source).not.toContain("rowSubject");
  });

  it("builds no menu markup at all", () => {
    const source = coreSourceOf("agentSurfacesRender.js");
    expect(source).not.toContain("menuButtonMarkup");
    expect(source).not.toContain("splitButton.js");
    expect(source).not.toContain("Ask");
  });
});

describe("the viewer that folds away what has finished", () => {
  const shellRows = (states) =>
    surfaceRows(SHELL_ENTRY_KIND, {
      shells: states.map((state, index) => ({ id: `s${index}`, description: `command ${index}`, state })),
    });

  const viewerOf = (states) => parseHtml(shellViewerHtml(shellRows(states))).querySelector(".surface-viewer");

  it("lists the running rows above the fold and the finished ones inside it", () => {
    const viewer = viewerOf(["running", "done", "failed"]);
    expect([...viewer.querySelectorAll(".surface-running > .surface-row")].map((row) => row.dataset.key)).toEqual([
      "s0",
    ]);
    expect(
      [...viewer.querySelectorAll(".surface-completed .surface-completed-rows > .surface-row")].map(
        (row) => row.dataset.key,
      ),
    ).toEqual(["s1", "s2"]);
  });

  it("says how many rows the fold holds, and keeps it shut until the reader opens it", () => {
    const fold = viewerOf(["running", "done", "failed"]).querySelector(".surface-completed");
    expect(fold.tagName).toBe("DETAILS");
    expect(fold.open).toBe(false);
    expect(fold.querySelector(".surface-completed-head").textContent.trim()).toBe("Completed (2)");
  });

  it("omits the fold entirely while nothing has finished", () => {
    expect(viewerOf(["running"]).querySelector(".surface-completed")).toBe(null);
    expect(viewerOf([]).querySelector(".surface-completed")).toBe(null);
  });

  it("paints both lists with the one row renderer of the kind", () => {
    const rows = shellRows(["running", "done"]);
    const html = shellViewerHtml(rows);
    expect(html).toContain(shellRowHtml(rows[0]));
    expect(html).toContain(shellRowHtml(rows[1]));
    const agents = agentRows([
      { id: "a1", label: "Reader", state: "running" },
      { id: "a2", label: "Counter", state: "done" },
    ]);
    const agentHtml = subagentViewerHtml(agents);
    expect(agentHtml).toContain(agentRowHtml(agents[0]));
    expect(agentHtml).toContain(agentRowHtml(agents[1]));
  });

  it("leaves the mount one frame with an empty running list and no fold", () => {
    const empty = parseHtml(shellViewerHtml([]));
    expect(empty.children.length).toBe(1);
    expect(empty.querySelector(".surface-viewer.surface-shells")).toBeTruthy();
    expect(empty.querySelector(".surface-running").children.length).toBe(0);
  });

  it("names each list a keyed paint fills", () => {
    const viewer = viewerOf(["running", "done"]);
    expect(viewer.querySelector(SURFACE_SELECTOR.running)).toBeTruthy();
    expect(viewer.querySelector(SURFACE_SELECTOR.completed)).toBeTruthy();
    expect(viewer.querySelector(COMPLETED_FOLD_SELECTOR)).toBeTruthy();
    expect(parseHtml(completedFoldHtml(2)).querySelector(COMPLETED_FOLD_HEAD_SELECTOR).textContent.trim()).toBe(
      "Completed (2)",
    );
  });
});
