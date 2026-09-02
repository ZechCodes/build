// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SPAWNING_CALL_SEQUENCE, surfacesSnapshot } from "./surfacesFixture.js";
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
  mountSurfaceViewer(host(), kind, { onOpenThreadItem: options.onOpenThreadItem || (() => {}) });

const runningRows = () => [...document.querySelectorAll(".surface-running > .surface-row")];

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

  it("chooses the phase pressed in a workflow, without leaving the viewer", () => {
    const viewer = mount(WORKFLOW_ENTRY_KIND);
    viewer.set(snapshot());
    const [, secondPhase] = [...document.querySelectorAll(".surface-phase")];

    secondPhase.click();

    expect(secondPhase.getAttribute("aria-pressed")).toBe("true");
    expect(document.querySelector(".surface-phase-agents").textContent).toContain("judge");
    viewer.dispose();
  });

  it("opens the thread item a subagent row was spawned by", () => {
    const onOpenThreadItem = vi.fn();
    const viewer = mount(AGENT_ENTRY_KIND, { onOpenThreadItem });
    viewer.set(snapshot());

    document.querySelector(".surface-completed-rows > .surface-row").click();

    expect(onOpenThreadItem.mock.calls).toEqual([[SPAWNING_CALL_SEQUENCE]]);
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
    row.click();
    expect(onOpenThreadItem).not.toHaveBeenCalled();
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

  it("shows a finished row its duration and no clock, and a row with neither nothing", () => {
    const viewer = mount(AGENT_ENTRY_KIND);
    viewer.set(
      ticking({
        subagents: [
          { id: "s1", label: "parser reviewer", state: "done", duration_ms: 65_000, started_at: LAUNCHED_AT },
          { id: "s2", label: "fixture writer", state: "running" },
        ],
      }),
    );

    expect(clocks()).toEqual([]);
    expect(document.querySelector(".surface-completed-rows").textContent).toContain("1m 05s");
    expect(document.querySelector(".surface-running").textContent).not.toContain("1m 05s");
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
