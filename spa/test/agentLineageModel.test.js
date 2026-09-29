// Who made whom, and what that makes running (#216).
//
// An agent's activity panel lists the Build agents it created beside its
// harness sub-agents, and an agent with any running agent in that panel counts
// as running itself. The rollup is transitive: a middle agent whose own
// creation is running shows as running on its creator's panel, so its creator
// has a running agent in its panel too.

import { describe, expect, it } from "vitest";
import { agentLineage, buildAgentEntries, lineageMembers, withRollup } from "../src/core/agentLineageModel.js";

const row = (kind, over = {}) => ({ kind, project_id: "proj-1", run_id: `run-${over.workspace_id || kind}`, ...over });
const agent = (id, over = {}) => ({ id, ordinal: 1, working: false, ...over });
const runningSubagent = (startedAt) => ({ subagents: [{ id: "sub-1", label: "Explore", state: "running", started_at: startedAt }] });

const lineageOf = (rows) => agentLineage(lineageMembers(rows, { projectId: "proj-1" }));

describe("who an agent created", () => {
  const rows = [
    row("project", { agents: [agent("project-agent")] }),
    row("workspace", { workspace_id: "ws-a", title: "Fix login", agents: [agent("worker-a", { created_by: "project-agent", name: "Login fixer" })] }),
    row("workspace", { workspace_id: "ws-b", agents: [agent("worker-b", { created_by: "project-agent" }), agent("mine")] }),
    row("workspace", { project_id: "proj-2", workspace_id: "ws-x", agents: [agent("stranger", { created_by: "project-agent" })] }),
  ];

  it("lists the agents whose created_by names it, and only those", () => {
    expect(lineageOf(rows).createdBy("project-agent").map((one) => one.agent.id)).toEqual(["worker-a", "worker-b"]);
    expect(lineageOf(rows).createdBy("worker-a")).toEqual([]);
  });

  it("carries where each one lives, for the click that opens its chat", () => {
    const [first] = lineageOf(rows).createdBy("project-agent");
    expect(first).toMatchObject({ entityId: "run-ws-a", workspaceId: "ws-a", kind: "workspace" });
  });

  it("reads nothing from a project it was not asked about", () => {
    expect(lineageOf(rows).createdBy("project-agent").some((one) => one.agent.id === "stranger")).toBe(false);
  });

  it("answers nothing for a row cache with no creators in it", () => {
    expect(lineageOf([row("workspace", { agents: [agent("solo")] })]).createdBy("solo")).toEqual([]);
  });
});

describe("the running rollup", () => {
  it("counts an agent running on its own loop", () => {
    const lineage = lineageOf([row("workspace", { agents: [agent("a", { working: true })] })]);
    expect(lineage.rollup("a").running).toBe(true);
  });

  it("counts an idle agent running while an agent it created runs", () => {
    const lineage = lineageOf([
      row("project", { agents: [agent("boss")] }),
      row("workspace", { workspace_id: "ws-a", agents: [agent("worker", { created_by: "boss", working: true, working_time: { since: "2026-09-28T10:00:00Z" } })] }),
    ]);
    expect(lineage.rollup("boss")).toMatchObject({ running: true, agentsRunning: 1, since: "2026-09-28T10:00:00Z" });
  });

  it("counts an idle agent running while one of its harness sub-agents runs", () => {
    const lineage = lineageOf([row("workspace", { agents: [agent("a", { surfaces: runningSubagent(Date.parse("2026-09-28T09:00:00Z")) })] })]);
    expect(lineage.rollup("a")).toMatchObject({ running: true, agentsRunning: 1, since: "2026-09-28T09:00:00.000Z" });
  });

  it("is transitive: a grandchild running makes the whole line running", () => {
    const lineage = lineageOf([
      row("project", { agents: [agent("top")] }),
      row("workspace", { workspace_id: "ws-a", agents: [agent("middle", { created_by: "top" })] }),
      row("workspace", { workspace_id: "ws-b", agents: [agent("leaf", { created_by: "middle", working: true })] }),
    ]);
    expect(lineage.rollup("middle")).toMatchObject({ running: true, agentsRunning: 1 });
    expect(lineage.rollup("top")).toMatchObject({ running: true, agentsRunning: 1 });
  });

  it("is idle when nothing in the line runs", () => {
    const lineage = lineageOf([
      row("project", { agents: [agent("top")] }),
      row("workspace", { workspace_id: "ws-a", agents: [agent("child", { created_by: "top" })] }),
    ]);
    expect(lineage.rollup("top")).toEqual({ running: false, agentsRunning: 0, since: null });
  });

  it("keeps its own clock while its own loop runs", () => {
    const lineage = lineageOf([
      row("project", { agents: [agent("boss", { working: true, working_time: { since: "2026-09-28T11:00:00Z" } })] }),
      row("workspace", { workspace_id: "ws-a", agents: [agent("worker", { created_by: "boss", working: true, working_time: { since: "2026-09-28T10:00:00Z" } })] }),
    ]);
    expect(lineage.rollup("boss").since).toBe("2026-09-28T11:00:00Z");
  });

  it("survives a cycle no bridge should ever write", () => {
    const lineage = lineageOf([row("workspace", { agents: [
      agent("a", { created_by: "b" }),
      agent("b", { created_by: "a" }),
    ] })]);
    expect(lineage.rollup("a").running).toBe(false);
  });

  it("answers idle for an agent it has never seen", () => {
    expect(lineageOf([]).rollup("nobody")).toEqual({ running: false, agentsRunning: 0, since: null });
  });
});

describe("an agent with the rollup laid on", () => {
  const lineage = lineageOf([
    row("project", { agents: [agent("boss")] }),
    row("workspace", { workspace_id: "ws-a", agents: [agent("worker", { created_by: "boss", working: true })] }),
  ]);

  it("says how many agents in its panel are running, beside its own working", () => {
    const boss = withRollup(agent("boss"), lineage);
    expect(boss.working).toBe(false);
    expect(boss.agents_running).toBe(1);
  });

  it("is the same agent when nothing in its panel runs", () => {
    const quiet = agent("worker-2");
    expect(withRollup(quiet, lineage)).toBe(quiet);
  });
});

describe("the Build agents an agent's panel lists", () => {
  const lineage = lineageOf([
    row("project", { agents: [agent("boss")] }),
    row("workspace", { workspace_id: "ws-a", title: "Fix login", agents: [
      agent("worker", { created_by: "boss", name: "Login fixer" }),
    ] }),
    row("workspace", { workspace_id: "ws-b", title: "Docs", agents: [
      agent("busy", { created_by: "boss", ordinal: 2, working: true, working_time: { since: "2026-09-28T10:00:00Z" } }),
      agent("broken", { created_by: "boss", ordinal: 3, start_error: "Claude Code 2.1 cannot run Opus 5.5" }),
    ] }),
  ]);

  it("names each by what the rail calls it, with where its chat is", () => {
    expect(buildAgentEntries(lineage, "boss")).toEqual([
      { id: "worker", name: "Login fixer", state: "idle", started_at: null,
        entity_id: "run-ws-a", workspace_id: "ws-a", workspace_name: "Fix login" },
      { id: "busy", name: "Agent 2", state: "running", started_at: Date.parse("2026-09-28T10:00:00Z"),
        entity_id: "run-ws-b", workspace_id: "ws-b", workspace_name: "Docs" },
      { id: "broken", name: "Agent 3", state: "failed", started_at: null,
        entity_id: "run-ws-b", workspace_id: "ws-b", workspace_name: "Docs" },
    ]);
  });

  it("is none at all for an agent that made nothing, so no pill is put up for it", () => {
    expect(buildAgentEntries(lineage, "worker")).toBeNull();
    expect(buildAgentEntries(null, "boss")).toBeNull();
  });
});
