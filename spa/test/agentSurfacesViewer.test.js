// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SPAWNING_CALL_SEQUENCE, WORKFLOW_STARTED_AT, surfacesSnapshot } from "./surfacesFixture.js";
import { mountSurfaceViewer } from "../src/core/agentSurfaces.js";
import {
  AGENT_ENTRY_KIND,
  SHELL_ENTRY_KIND,
  WORKFLOW_ENTRY_KIND,
} from "../src/core/agentSurfacesModel.js";

const snapshot = () => surfacesSnapshot();

const host = () => {
  document.body.innerHTML = `<div class="overlay-host"></div>`;
  return document.querySelector(".overlay-host");
};

const mount = (kind, options = {}) =>
  mountSurfaceViewer(host(), kind, { onOpenThreadItem: () => {}, ...options });

const phaseSections = () => [...document.querySelectorAll(".surface-phase")];
const agentLabelsIn = (section) =>
  [...section.querySelectorAll(".surface-row-label")].map((label) => label.textContent);

const runningRows = () => [...document.querySelectorAll(".surface-running > .surface-row")];

const withModel = (model) =>
  surfacesSnapshot({
    workflows: [
      {
        id: "wf-1",
        name: "Review sweep",
        state: "running",
        phases: [{ title: "Read", agents: [{ id: "a1", label: "reader", state: "running", model }] }],
      },
    ],
  });

describe("mountSurfaceViewer", () => {
  it("paints the kind it was mounted for, with its rows and no menu on any of them", () => {
    const viewer = mount(SHELL_ENTRY_KIND);
    viewer.set(snapshot());

    expect(document.querySelector(".surface-shells")).not.toBe(null);
    expect(runningRows().map((row) => row.dataset.key)).toEqual(["sh1"]);
    expect(runningRows()[0].querySelector(".splitbtn")).toBe(null);
    viewer.dispose();
  });

  it("paints its frame with no rows for a kind the snapshot has nothing of", () => {
    const viewer = mount(SHELL_ENTRY_KIND);
    viewer.set({ subagents: snapshot().subagents });

    expect(document.querySelector(".surface-shells")).not.toBe(null);
    expect(runningRows()).toEqual([]);
    viewer.dispose();
  });

  it("adds the row a later snapshot brought, keeping the one already painted", () => {
    const viewer = mount(SHELL_ENTRY_KIND);
    viewer.set(snapshot());
    const [first] = runningRows();

    const grown = snapshot();
    grown.shells.push({ id: "sh2", description: "cargo clippy", state: "running", tail: [] });
    viewer.set(grown);

    expect(runningRows()).toHaveLength(2);
    expect(runningRows()[0]).toBe(first);
    expect(runningRows()[1].textContent).toContain("cargo clippy");
    viewer.dispose();
  });

  it("draws no menu on a workflow head either", () => {
    const viewer = mount(WORKFLOW_ENTRY_KIND);
    viewer.set(snapshot());

    expect(document.querySelector(".surface-workflow-head .splitbtn")).toBe(null);
    expect(document.querySelectorAll(".splitmenu")).toHaveLength(0);
    viewer.dispose();
  });

  it("stacks a section per phase, each holding its own agents, the running one open", () => {
    const viewer = mount(WORKFLOW_ENTRY_KIND);
    viewer.set(snapshot());

    const sections = phaseSections();
    expect(sections.map((section) => section.tagName)).toEqual(["DETAILS", "DETAILS"]);
    expect(sections.map((section) => section.open)).toEqual([true, false]);
    expect(sections.map(agentLabelsIn)).toEqual([["reader"], ["judge"]]);
    viewer.dispose();
  });

  it("opens the phase that starts running later, and leaves a collapsed one collapsed", () => {
    const viewer = mount(WORKFLOW_ENTRY_KIND);
    viewer.set(snapshot());
    expect(phaseSections().map((section) => section.open)).toEqual([true, false]);

    const moved = snapshot();
    moved.workflows[0].phases[0].agents[0].state = "done";
    moved.workflows[0].phases[1].agents[0].state = "running";
    viewer.set(moved);

    expect(phaseSections().map((section) => section.open)).toEqual([true, true]);

    phaseSections()[1].open = false;
    viewer.set(moved);

    expect(phaseSections()[1].open).toBe(false);
    viewer.dispose();
  });

  it("leaves a section the reader toggled where the reader put it, repaint after repaint", () => {
    const viewer = mount(WORKFLOW_ENTRY_KIND);
    viewer.set(snapshot());
    const [running, pending] = phaseSections();
    running.open = false;
    pending.open = true;

    const moved = snapshot();
    moved.workflows[0].phases[0].agents.push({ id: "a3", label: "second reader", state: "queued" });
    viewer.set(moved);

    expect(phaseSections()).toEqual([running, pending]);
    expect(phaseSections().map((section) => section.open)).toEqual([false, true]);
    viewer.dispose();
  });

  it("gives a press on a phase title to the fold, the whole of the title reading on hover", () => {
    const viewer = mount(WORKFLOW_ENTRY_KIND);
    viewer.set(snapshot());
    const [, pending] = phaseSections();
    const title = pending.querySelector(".surface-phase-title");

    title.click();

    expect(pending.open).toBe(true);
    expect(title.hasAttribute("data-expanded")).toBe(false);
    expect(title.getAttribute("title")).toBe("Judge");
    viewer.dispose();
  });

  it("names a model through the label its caller hands it, and by its id without one", () => {
    const named = mount(WORKFLOW_ENTRY_KIND, { modelLabel: (id) => `Opus 5 · ${id}` });
    named.set(withModel("claude-opus-5"));
    expect(document.querySelector(".surface-row-model").textContent).toBe("Opus 5 · claude-opus-5");
    named.dispose();

    const raw = mount(WORKFLOW_ENTRY_KIND);
    raw.set(withModel("claude-opus-5"));
    expect(document.querySelector(".surface-row-model").textContent).toBe("claude-opus-5");
    raw.dispose();
  });

  it("keeps stats available inside both compact and roomy row disclosures", () => {
    const counted = { id: "a1", label: "reader", state: "running", model: "haiku", tokens: 1200, tool_calls: 4 };
    const compact = mount(AGENT_ENTRY_KIND, { compact: true });
    compact.set(surfacesSnapshot({ subagents: [counted] }));
    expect(document.querySelector(".surface-agent-facts").textContent).toContain("1200");
    compact.dispose();

    const roomy = mount(AGENT_ENTRY_KIND);
    roomy.set(surfacesSnapshot({ subagents: [counted] }));
    expect(document.querySelector(".surface-agent-facts").textContent).toContain("1200");
    roomy.dispose();
  });

  it("opens the thread item a subagent row was spawned by from the row's own control", () => {
    const onOpenThreadItem = vi.fn();
    const viewer = mount(AGENT_ENTRY_KIND, { onOpenThreadItem });
    viewer.set(snapshot());
    const row = document.querySelector(".surface-completed-rows > .surface-row");

    row.querySelector(".surface-agent-summary").click();

    expect(onOpenThreadItem).not.toHaveBeenCalled();
    expect(row.open).toBe(true);

    row.querySelector("[data-call-sequence]").click();

    expect(onOpenThreadItem.mock.calls).toEqual([[SPAWNING_CALL_SEQUENCE]]);
    viewer.dispose();
  });

  it("keeps an expanded subagent open through a repaint and its move into Completed", async () => {
    const viewer = mount(AGENT_ENTRY_KIND);
    const running = snapshot();
    running.subagents[1] = {
      ...running.subagents[1],
      description: "Review the fixture writer",
      model: "gpt-5.6-sol",
      reasoning_effort: "high",
      tokens: 1200,
      tool_calls: 4,
    };
    viewer.set(running);
    const row = document.querySelector('.surface-running > [data-key="s2"]');

    row.querySelector(".surface-agent-summary").click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(row.open).toBe(true);

    const progressed = snapshot();
    progressed.subagents[1] = { ...running.subagents[1], tokens: 1400 };
    viewer.set(progressed);
    expect(document.querySelector('.surface-running > [data-key="s2"]')).toBe(row);
    expect(row.open).toBe(true);
    expect(row.querySelector(".surface-agent-facts").textContent).toContain("1400");

    const finished = snapshot();
    finished.subagents[1] = { ...progressed.subagents[1], state: "done", result: "Fixture review complete" };
    viewer.set(finished);
    const moved = document.querySelector('.surface-completed-rows > [data-key="s2"]');
    expect(moved).not.toBe(row);
    expect(moved.open).toBe(true);
    expect(moved.closest(".surface-completed").open).toBe(true);
    expect(moved.querySelector(".surface-row-detail").textContent).toBe("Fixture review complete");
    viewer.dispose();
  });

  it("empties its host on dispose and hears nothing more from it", () => {
    const onOpenThreadItem = vi.fn();
    const viewer = mount(AGENT_ENTRY_KIND, { onOpenThreadItem });
    viewer.set(snapshot());
    const [row] = [...document.querySelectorAll(".surface-completed-rows > .surface-row")];
    const mountedHost = document.querySelector(".overlay-host");

    viewer.dispose();

    expect(mountedHost.innerHTML).toBe("");
    mountedHost.appendChild(row);
    row.querySelector("[data-call-sequence]").click();
    expect(onOpenThreadItem).not.toHaveBeenCalled();
  });
});

describe("the text a viewer clips", () => {
  const clipped = () => document.querySelector(".surface-row-label");

  it("opens on a press and clips back on the next one, the title always carrying all of it", () => {
    const viewer = mount(SHELL_ENTRY_KIND);
    viewer.set(surfacesSnapshot({ shells: [{ id: "sh1", description: "cargo test --all-features", state: "running" }] }));

    expect(clipped().getAttribute("title")).toBe("cargo test --all-features");
    expect(clipped().hasAttribute("data-expanded")).toBe(false);

    clipped().click();
    expect(clipped().hasAttribute("data-expanded")).toBe(true);

    clipped().click();
    expect(clipped().hasAttribute("data-expanded")).toBe(false);
    viewer.dispose();
  });

  it("opens from the keyboard as it does from a press, its aria saying which way it stands", () => {
    const viewer = mount(SHELL_ENTRY_KIND);
    viewer.set(surfacesSnapshot({ shells: [{ id: "sh1", description: "cargo test", state: "running" }] }));
    const press = (key) => clipped().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));

    expect(clipped().getAttribute("role")).toBe("button");
    expect(clipped().tabIndex).toBe(0);
    expect(clipped().getAttribute("aria-expanded")).toBe("false");

    press("Enter");
    expect(clipped().hasAttribute("data-expanded")).toBe(true);
    expect(clipped().getAttribute("aria-expanded")).toBe("true");

    press(" ");
    expect(clipped().hasAttribute("data-expanded")).toBe(false);
    expect(clipped().getAttribute("aria-expanded")).toBe("false");
    viewer.dispose();
  });

  it("stays open across a repaint that moved the row around it", () => {
    const viewer = mount(SHELL_ENTRY_KIND);
    viewer.set(surfacesSnapshot({ shells: [{ id: "sh1", description: "cargo test", state: "running", tail: [] }] }));
    clipped().click();
    const expanded = clipped();

    viewer.set(surfacesSnapshot({ shells: [{ id: "sh1", description: "cargo test", state: "running", tail: ["one"] }] }));

    expect(clipped()).toBe(expanded);
    expect(clipped().hasAttribute("data-expanded")).toBe(true);
    viewer.dispose();
  });
});

describe("the clock a running row ticks", () => {
  const LAUNCHED_AT = 1788291725678;

  const ticking = (overrides = {}) =>
    surfacesSnapshot({
      subagents: [{ id: "s2", label: "fixture writer", state: "running", started_at: LAUNCHED_AT }],
      shells: [{ id: "sh1", description: "cargo test", state: "running", started_at: LAUNCHED_AT, tail: [] }],
      ...overrides,
    });

  const clocks = () => [...document.querySelectorAll(".surface-row-clock")].map((span) => span.textContent);
  const tickingClocks = () => [...document.querySelectorAll("[data-running-since]")];

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(LAUNCHED_AT + 65_000);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("reads the elapsed time of a running agent row and a running shell row", () => {
    const agents = mount(AGENT_ENTRY_KIND);
    agents.set(ticking());
    expect(clocks()).toEqual(["1:05"]);
    agents.dispose();

    const shells = mount(SHELL_ENTRY_KIND);
    shells.set(ticking());
    expect(clocks()).toEqual(["1:05"]);
    shells.dispose();
  });

  it("advances on the next tick without replacing the row it is in", () => {
    const viewer = mount(SHELL_ENTRY_KIND);
    viewer.set(ticking());
    const [row] = runningRows();

    vi.advanceTimersByTime(1000);

    expect(clocks()).toEqual(["1:06"]);
    expect(runningRows()[0]).toBe(row);
    viewer.dispose();
  });

  it("shows a finished row what it took in the slot it ticked in, and a row with neither nothing", () => {
    const viewer = mount(AGENT_ENTRY_KIND);
    viewer.set(
      ticking({
        subagents: [
          { id: "s1", label: "parser reviewer", state: "done", duration_ms: 65_000, started_at: LAUNCHED_AT },
          { id: "s2", label: "fixture writer", state: "running" },
        ],
      }),
    );

    expect(clocks()).toEqual(["1:05"]);
    expect(tickingClocks()).toEqual([]);
    expect(document.querySelector(".surface-running").textContent).not.toContain("1:05");
    viewer.dispose();
  });

  it("ticks the phase's own clock alongside the rows inside it", () => {
    const viewer = mount(WORKFLOW_ENTRY_KIND);
    viewer.set(
      surfacesSnapshot({
        workflows: [
          {
            id: "wf-1",
            name: "Review sweep",
            state: "running",
            phases: [
              { title: "Read", agents: [{ id: "a1", label: "reader", state: "running", started_at: LAUNCHED_AT }] },
            ],
          },
        ],
      }),
    );

    expect(clocks()).toEqual(["1:05", "1:05"]);
    vi.advanceTimersByTime(1000);
    expect(clocks()).toEqual(["1:06", "1:06"]);
    viewer.dispose();
  });

  it("stops ticking when the last running row leaves, and on dispose", () => {
    const started = vi.spyOn(globalThis, "setInterval");
    const stopped = vi.spyOn(globalThis, "clearInterval");
    const viewer = mount(SHELL_ENTRY_KIND);
    viewer.set(ticking());
    expect(started).toHaveBeenCalledTimes(1);

    viewer.set(ticking({ shells: [{ id: "sh1", description: "cargo test", state: "done", tail: [] }] }));
    expect(stopped).toHaveBeenCalledWith(started.mock.results[0].value);

    viewer.set(ticking());
    expect(started).toHaveBeenCalledTimes(2);
    viewer.dispose();
    expect(stopped).toHaveBeenCalledWith(started.mock.results[1].value);
  });
});
