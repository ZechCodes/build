// Who made whom, and what that makes running (#216).
//
// An agent's activity panel lists the Build agents it created beside its
// harness sub-agents, and an agent with any running agent in that panel counts
// as running itself. The rollup is transitive: a middle agent whose own
// creation is running shows as running on its creator's panel, so its creator
// has a running agent in its panel too.

import { describe, expect, it } from "vitest";
import { agentLineage, buildAgentChatRoute, buildAgentEntries, lineageMembers, withRollup } from "../src/core/agentLineageModel.js";
import { conversationRoute } from "../src/core/router.js";

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

  // #221: a memo filled while a cycle was cut short must not make the answer
  // depend on which agent the rail happened to ask about first.
  it("answers a cycle the same whichever agent is asked about first", () => {
    const cycle = () => lineageOf([row("workspace", { agents: [
      agent("a", { created_by: "b" }),
      agent("b", { created_by: "a", working: true, working_time: { since: "2026-09-28T10:00:00Z" } }),
      agent("c", { created_by: "a", working: true, working_time: { since: "2026-09-28T09:00:00Z" } }),
    ] })]);
    const ids = ["a", "b", "c"];
    const answersAskedFrom = (first) => {
      const lineage = cycle();
      lineage.rollup(first);
      return Object.fromEntries(ids.map((id) => [id, lineage.rollup(id)]));
    };
    const answers = ids.map(answersAskedFrom);
    expect(answers[1]).toEqual(answers[0]);
    expect(answers[2]).toEqual(answers[0]);
    // Agents on the cycle are made by no one for the rollup: a counts c, which
    // it made off the cycle, and not b, whose line of makers leads back to it.
    expect(answers[0].a).toEqual({ running: true, agentsRunning: 1, since: "2026-09-28T09:00:00Z" });
    expect(answers[0].b).toEqual({ running: true, agentsRunning: 0, since: "2026-09-28T10:00:00Z" });
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
        entity_id: "run-ws-a", workspace_id: "ws-a", workspace_name: "Fix login", kind: "workspace", model: "", provider: "" },
      { id: "busy", name: "Agent 2", state: "running", started_at: Date.parse("2026-09-28T10:00:00Z"),
        entity_id: "run-ws-b", workspace_id: "ws-b", workspace_name: "Docs", kind: "workspace", model: "", provider: "" },
      { id: "broken", name: "Agent 3", state: "failed", started_at: null,
        entity_id: "run-ws-b", workspace_id: "ws-b", workspace_name: "Docs", kind: "workspace", model: "", provider: "" },
    ]);
  });

  it("is none at all for an agent that made nothing, so no pill is put up for it", () => {
    expect(buildAgentEntries(lineage, "worker")).toBeNull();
    expect(buildAgentEntries(null, "boss")).toBeNull();
  });
});

// #221: the chat a Build agent's row opens is the one every other link to an
// agent's conversation opens, and a row whose chat has no page opens nothing.
describe("where a press on a Build agent goes", () => {
  const place = { deviceId: "dev-1", projectId: "proj-1" };

  it("is the conversation route of a workspace's agent", () => {
    expect(buildAgentChatRoute({ agentId: "worker", kind: "workspace", workspaceId: "ws-a" }, place))
      .toEqual(conversationRoute({ kind: "workspace", projectId: "proj-1", deviceId: "dev-1", workspaceId: "ws-a", agentId: "worker" }));
  });

  it("is the project's conversation route for an agent with no workspace", () => {
    expect(buildAgentChatRoute({ agentId: "deputy", kind: "project", workspaceId: null }, place))
      .toEqual(conversationRoute({ kind: "project", projectId: "proj-1", deviceId: "dev-1", agentId: "deputy" }));
  });

  it("is nowhere for an agent on neither", () => {
    expect(buildAgentChatRoute({ agentId: "stray", kind: "branch", workspaceId: null }, place)).toBeNull();
    expect(buildAgentChatRoute({ agentId: "lost", kind: "workspace", workspaceId: null }, place)).toBeNull();
  });
});

// #226: the bridge files every agent on its run's branch row
// (`board.list`'s `branch_candidate_from_run`) — a workspace's and the
// project agent's included. Neither row says where it lives; the cached
// workspace and project lists do, by naming the row's run as their own.
describe("where a Build agent lives, off the rows the bridge actually writes", () => {
  const runRow = (runId, agents, over = {}) => ({
    kind: "branch", project_id: "proj-1", run_id: runId, branch: `build/${runId}`, title: `goal of ${runId}`,
    worktree_path: `/work/${runId}`, task_id: null, agents, ...over,
  });
  const workspaces = [{ id: "ws-skrift", project_id: "proj-1", name: "Skrift 0.2.1 validation", entity_id: "run-ws", run_id: "run-ws", root: "/work/run-ws" }];
  const projects = [{ project_id: "proj-1", name: "Build", entity_id: "run-project", run_id: "run-project" }];
  const rows = [
    runRow("run-project", [agent("boss"), agent("deputy", { created_by: "boss", ordinal: 2 })]),
    runRow("run-ws", [agent("validator", { created_by: "boss", name: "Skrift validation" })]),
    runRow("run-branch", [agent("brancher", { created_by: "boss" })]),
  ];
  const members = lineageMembers(rows, { projectId: "proj-1", workspaces, projects });
  const entries = buildAgentEntries(agentLineage(members), "boss");
  const entryOf = (id) => entries.find((entry) => entry.id === id);

  it("is the workspace whose run the row is, named as the workspace is", () => {
    expect(entryOf("validator")).toMatchObject({
      kind: "workspace", workspace_id: "ws-skrift", workspace_name: "Skrift 0.2.1 validation", entity_id: "run-ws",
    });
    expect(buildAgentChatRoute({ agentId: "validator", kind: entryOf("validator").kind, workspaceId: "ws-skrift" }, { deviceId: "dev-1", projectId: "proj-1" }))
      .not.toBeNull();
  });

  it("is the project for an agent on the project agent's own conversation", () => {
    expect(entryOf("deputy")).toMatchObject({ kind: "project", workspace_id: null });
  });

  it("is still the branch for an agent on a branch no workspace or project owns", () => {
    expect(entryOf("brancher")).toMatchObject({ kind: "branch", workspace_id: null, workspace_name: "goal of run-branch" });
  });

  it("reads the lists as optional, so a cache holding only rows answers as before", () => {
    expect(lineageMembers(rows, { projectId: "proj-1" }).map((member) => member.kind)).toEqual(["branch", "branch", "branch", "branch"]);
  });
});

describe("a Build agent row that names its workspace outright", () => {
  it("opens on that workspace whatever kind of row carried it", () => {
    expect(buildAgentChatRoute({ agentId: "worker", kind: "task", workspaceId: "ws-a" }, { deviceId: "dev-1", projectId: "proj-1" }))
      .toEqual(conversationRoute({ kind: "workspace", projectId: "proj-1", deviceId: "dev-1", workspaceId: "ws-a", agentId: "worker" }));
  });
});

describe("the model a Build agent runs", () => {
  const entriesFor = (over) => buildAgentEntries(lineageOf([
    row("project", { agents: [agent("boss")] }),
    row("workspace", { workspace_id: "ws-a", agents: [agent("worker", { created_by: "boss", provider: "codex_app_server", ...over })] }),
  ]), "boss");

  it("is the model it is running now, with the harness that names it", () => {
    expect(entriesFor({ model: "gpt-6-sol", active_model: "gpt-6-astra" })[0]).toMatchObject({ model: "gpt-6-astra", provider: "codex_app_server" });
  });

  it("is the model it will start on while it has not run", () => {
    expect(entriesFor({ model: "gpt-6-sol", active_model: "" })[0]).toMatchObject({ model: "gpt-6-sol" });
  });

  it("is nothing for an agent whose digest names none", () => {
    expect(entriesFor({})[0]).toMatchObject({ model: "" });
  });
});
