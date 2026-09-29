// @vitest-environment jsdom
// The Agents viewer's two groups (#216): the harness's sub-agents, and the
// Build agents this agent made through the Build MCP. A press on a Build agent
// opens that agent's chat.
import { afterEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountSurfaceViewer } from "../src/core/agentSurfaces.js";
import { AGENT_ENTRY_KIND, BUILD_AGENTS_KEY } from "../src/core/agentSurfacesModel.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const subagent = (id, state) => ({ id, label: `Sub ${id}`, state });
const buildAgent = (id, state, over = {}) => ({
  id, name: `Worker ${id}`, state, kind: "workspace", entity_id: `run-${id}`, workspace_id: `ws-${id}`, workspace_name: `Space ${id}`, ...over,
});

let viewer = null;
const mount = (options = {}) => {
  document.body.innerHTML = `<div class="overlay-host"></div>`;
  viewer = mountSurfaceViewer(document.querySelector(".overlay-host"), AGENT_ENTRY_KIND, { onOpenThreadItem: () => {}, ...options });
  return viewer;
};

afterEach(() => {
  viewer?.dispose();
  viewer = null;
});

const group = (name) => document.querySelector(`.surface-group[data-group="${name}"]`);
const headOf = (name) => group(name)?.querySelector(".surface-group-head")?.textContent.trim();
const buildRows = () => [...document.querySelectorAll(".surface-build-agents > .surface-build-agent")];

describe("the Agents viewer's groups", () => {
  it("heads the sub-agents and the Build agents apart", () => {
    mount().set({
      subagents: [subagent("s1", "running")],
      [BUILD_AGENTS_KEY]: [buildAgent("b1", "idle")],
    });
    expect(headOf("subagents")).toBe("Sub-agents");
    expect(headOf(BUILD_AGENTS_KEY)).toBe("Build agents");
    expect(group("subagents").hidden).toBe(false);
    expect(group(BUILD_AGENTS_KEY).hidden).toBe(false);
    expect(group("subagents").querySelector(".surface-running .surface-agent").dataset.key).toBe("s1");
    expect(buildRows().map((row) => row.dataset.buildAgent)).toEqual(["b1"]);
  });

  it("draws no Build agents group for an agent that made none", () => {
    mount().set({ subagents: [subagent("s1", "running")] });
    expect(group(BUILD_AGENTS_KEY).hidden).toBe(true);
    expect(group("subagents").hidden).toBe(false);
  });

  it("draws no Sub-agents group for an agent with only Build agents", () => {
    mount().set({ [BUILD_AGENTS_KEY]: [buildAgent("b1", "running")] });
    expect(group("subagents").hidden).toBe(true);
    expect(group(BUILD_AGENTS_KEY).hidden).toBe(false);
  });

  it("keeps a finished sub-agent's history inside the Sub-agents group", () => {
    mount().set({
      subagents: [subagent("s1", "done")],
      [BUILD_AGENTS_KEY]: [buildAgent("b1", "idle")],
    });
    expect(group("subagents").hidden).toBe(false);
    expect(group("subagents").querySelector(".surface-completed")).not.toBe(null);
  });

  it("names each Build agent, its workspace and where it stands, running first", () => {
    mount().set({ [BUILD_AGENTS_KEY]: [buildAgent("quiet", "idle"), buildAgent("busy", "running", { started_at: Date.now() })] });
    const [first, second] = buildRows();
    expect(first.dataset.buildAgent).toBe("busy");
    expect(first.querySelector(".surface-row-label").textContent).toBe("Worker busy");
    expect(first.querySelector('[data-outcome="running"]').getAttribute("aria-label")).toBe("Working");
    expect(first.querySelector("[data-running-since]")).not.toBe(null);
    expect(second.querySelector(".surface-build-agent-where").textContent).toBe("Space quiet");
    expect(second.querySelector('[data-outcome="pending"]').getAttribute("aria-label")).toBe("Idle");
  });
});

describe("a press on a Build agent", () => {
  it("opens that agent's chat, saying where it lives", () => {
    const onOpenBuildAgent = vi.fn();
    mount({ onOpenBuildAgent }).set({ [BUILD_AGENTS_KEY]: [buildAgent("b1", "idle")] });
    buildRows()[0].querySelector(".surface-row-label").click();
    expect(onOpenBuildAgent).toHaveBeenCalledWith({ agentId: "b1", entityId: "run-b1", workspaceId: "ws-b1", kind: "workspace" });
  });

  it("is a button, reachable from the keyboard and titled with where it goes", () => {
    mount({ onOpenBuildAgent: () => {} }).set({ [BUILD_AGENTS_KEY]: [buildAgent("b1", "idle")] });
    const [row] = buildRows();
    expect(row.tagName).toBe("BUTTON");
    expect(row.getAttribute("title")).toBe("Open Worker b1's chat");
  });

  it("opens a project-level agent's chat on the project, having no workspace", () => {
    const onOpenBuildAgent = vi.fn();
    mount({ onOpenBuildAgent }).set({ [BUILD_AGENTS_KEY]: [buildAgent("p1", "idle", { kind: "project", workspace_id: null, workspace_name: "" })] });
    buildRows()[0].click();
    expect(onOpenBuildAgent).toHaveBeenCalledWith({ agentId: "p1", entityId: "run-p1", workspaceId: null, kind: "project" });
  });

  // #221: a Build agent with no page to open its chat on is drawn as what it
  // is — not a button, no pointer, and a title that says why.
  it("is not pressable when its chat has no page to open on, and says why", () => {
    const onOpenBuildAgent = vi.fn();
    mount({ onOpenBuildAgent }).set({ [BUILD_AGENTS_KEY]: [buildAgent("b1", "idle", { kind: "branch", workspace_id: null })] });
    const [row] = buildRows();
    expect(row.tagName).not.toBe("BUTTON");
    expect(row.hasAttribute("role")).toBe(false);
    expect(row.hasAttribute("tabindex")).toBe(false);
    expect(row.hasAttribute("data-build-agent")).toBe(false);
    expect(row.classList.contains("surface-build-agent-unreachable")).toBe(true);
    expect(row.getAttribute("title")).toBe("Build cannot open Worker b1's chat from here: it is on neither a workspace nor the project.");
    row.querySelector(".surface-row-label").click();
    expect(onOpenBuildAgent).not.toHaveBeenCalled();
  });
});
