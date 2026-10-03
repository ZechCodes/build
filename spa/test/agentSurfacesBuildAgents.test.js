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

  it("hides the Sub-agents heading while its only rows are in closed history", () => {
    const historyControl = document.createElement("button");
    mount({ historyControl }).set({
      subagents: [subagent("s1", "done")],
      [BUILD_AGENTS_KEY]: [buildAgent("b1", "running")],
    });
    expect(group("subagents").querySelector(".surface-running .surface-agent")).toBe(null);
    expect(group("subagents").querySelector(".surface-completed").open).toBe(false);
    expect(group("subagents").hidden).toBe(true);
    expect(group(BUILD_AGENTS_KEY).hidden).toBe(false);

    historyControl.click();
    expect(group("subagents").hidden).toBe(false);
    expect(group("subagents").querySelector(".surface-completed").open).toBe(true);
    historyControl.click();
    expect(group("subagents").hidden).toBe(true);
  });

  it("keeps a finished sub-agent's history inside the Sub-agents group", () => {
    mount().set({
      subagents: [subagent("s1", "done")],
      [BUILD_AGENTS_KEY]: [buildAgent("b1", "idle")],
    });
    expect(group("subagents").hidden).toBe(false);
    expect(group("subagents").querySelector(".surface-completed")).not.toBe(null);
  });

  it("names each Build agent and where it stands, running first", () => {
    mount().set({ [BUILD_AGENTS_KEY]: [buildAgent("quiet", "idle"), buildAgent("busy", "running", { started_at: Date.now() })] });
    const [first, second] = buildRows();
    expect(first.dataset.buildAgent).toBe("busy");
    expect(first.querySelector(".surface-row-label").textContent).toBe("Worker busy");
    expect(first.querySelector('[data-outcome="running"]').getAttribute("aria-label")).toBe("Working");
    expect(first.querySelector("[data-running-since]")).not.toBe(null);
    // #257: the workspace is where the row opens and what its title says,
    // not another thing on the row.
    expect(second.textContent).not.toContain("Space quiet");
    expect(second.querySelector(".surface-build-agent-where")).toBeNull();
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
    expect(row.getAttribute("title")).toBe("Open Worker b1's chat in Space b1");
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
    expect(row.getAttribute("title")).toBe("Worker b1 works on a branch outside any workspace. Its chat opens from that branch, not from here.");
    row.querySelector(".surface-row-label").click();
    expect(onOpenBuildAgent).not.toHaveBeenCalled();
  });

  // #226: a row of any other kind has nowhere at all, and says only that.
  it("says plainly when a Build agent's chat has nowhere to open", () => {
    mount({ onOpenBuildAgent: () => {} }).set({ [BUILD_AGENTS_KEY]: [buildAgent("t1", "idle", { kind: "task", workspace_id: null })] });
    const [row] = buildRows();
    expect(row.tagName).toBe("DIV");
    expect(row.getAttribute("title")).toBe("Build has nowhere to open Worker t1's chat yet.");
    expect(row.getAttribute("title")).not.toMatch(/neither/);
  });

  // #223: a branch has no page, but its own rail is where its agents' chats
  // are — there a branch agent's row opens in place, as before #221.
  describe("on a branch's own rail", () => {
    const onBranch = (id, entityId) => buildAgent(id, "idle", { kind: "branch", workspace_id: null, workspace_name: "", entity_id: entityId });

    it("is a button that opens the agent here", () => {
      const onOpenBuildAgent = vi.fn();
      mount({ onOpenBuildAgent, hereEntityId: () => "branch-a" }).set({ [BUILD_AGENTS_KEY]: [onBranch("b1", "branch-a")] });
      const [row] = buildRows();
      expect(row.tagName).toBe("BUTTON");
      expect(row.getAttribute("title")).toBe("Open Worker b1's chat");
      row.querySelector(".surface-row-label").click();
      expect(onOpenBuildAgent).toHaveBeenCalledWith({ agentId: "b1", entityId: "branch-a", workspaceId: null, kind: "here" });
    });

    it("leaves another branch's agent unpressable, saying why", () => {
      const onOpenBuildAgent = vi.fn();
      mount({ onOpenBuildAgent, hereEntityId: () => "branch-a" }).set({ [BUILD_AGENTS_KEY]: [onBranch("b2", "branch-b")] });
      const [row] = buildRows();
      expect(row.tagName).toBe("DIV");
      expect(row.getAttribute("title")).toBe("Worker b2 works on a branch outside any workspace. Its chat opens from that branch, not from here.");
      row.click();
      expect(onOpenBuildAgent).not.toHaveBeenCalled();
    });

    it("leaves workspace and project agents opening on their own pages", () => {
      const onOpenBuildAgent = vi.fn();
      mount({ onOpenBuildAgent, hereEntityId: () => "branch-a" }).set({ [BUILD_AGENTS_KEY]: [
        buildAgent("w1", "idle", { entity_id: "branch-a" }),
        buildAgent("p1", "idle", { kind: "project", workspace_id: null, workspace_name: "", entity_id: "branch-a" }),
      ] });
      for (const row of buildRows()) row.click();
      expect(onOpenBuildAgent.mock.calls.map(([where]) => [where.agentId, where.kind])).toEqual([["w1", "workspace"], ["p1", "project"]]);
    });

    it("reads the rail's entity at paint, so a rail that learns its entity later opens in place", () => {
      let here = null;
      mount({ onOpenBuildAgent: () => {}, hereEntityId: () => here }).set({ [BUILD_AGENTS_KEY]: [onBranch("b1", "branch-a")] });
      expect(buildRows()[0].tagName).toBe("DIV");
      here = "branch-a";
      viewer.set({ [BUILD_AGENTS_KEY]: [onBranch("b1", "branch-a")] });
      expect(buildRows()[0].tagName).toBe("BUTTON");
    });
  });
});

// #226: each Build agent row says what model it runs, labelled the way the
// rail labels a model: by the harness that agent runs on.
describe("the model on a Build agent's row", () => {
  const modelLabel = (modelId, providerId) => `${modelId} on ${providerId}`;
  const modelOf = (row) => row.querySelector(".surface-row-model")?.textContent;

  it("is the agent's model by its short name, the full label for its own harness in its title", () => {
    mount({ onOpenBuildAgent: () => {}, modelLabel }).set({ [BUILD_AGENTS_KEY]: [
      buildAgent("b1", "idle", { model: "gpt-6-astra", provider: "codex_app_server" }),
    ] });
    expect(modelOf(buildRows()[0])).toBe("6 Astra");
    expect(buildRows()[0].querySelector(".surface-row-model").classList.contains("surface-row-model-raw")).toBe(false);
    expect(buildRows()[0].querySelector(".surface-row-model").title).toBe("gpt-6-astra on codex_app_server");
  });

  it("keeps the full label for a model no short name is known for", () => {
    mount({ onOpenBuildAgent: () => {}, modelLabel }).set({ [BUILD_AGENTS_KEY]: [
      buildAgent("b1", "idle", { model: "llama-3", provider: "pi" }),
    ] });
    expect(modelOf(buildRows()[0])).toBe("llama-3 on pi");
    // #257: only a model with no short name may wrap; a short one stays on one line.
    expect(buildRows()[0].querySelector(".surface-row-model").classList.contains("surface-row-model-raw")).toBe(true);
  });

  it("shows on a row nothing opens, too", () => {
    mount({ modelLabel }).set({ [BUILD_AGENTS_KEY]: [
      buildAgent("b1", "idle", { kind: "branch", workspace_id: null, model: "claude-opus-5-5", provider: "claude" }),
    ] });
    expect(modelOf(buildRows()[0])).toBe("Opus 5.5");
  });

  it("is absent for an agent whose digest names no model", () => {
    mount({ onOpenBuildAgent: () => {}, modelLabel }).set({ [BUILD_AGENTS_KEY]: [buildAgent("b1", "idle")] });
    expect(buildRows()[0].querySelector(".surface-row-model")).toBeNull();
  });
});
