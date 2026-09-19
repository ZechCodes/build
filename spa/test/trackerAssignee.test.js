// The assignee picker's model. Assigning IS dispatching, so every option says
// what it is about to do, and one tagged field carries all five kinds.

import { describe, expect, it } from "vitest";
import {
  AGENT_FORM,
  WORKSPACE_FORM,
  agentLabels,
  assignParams,
  assigneeFor,
  assigneeOptions,
  selectedOptionId,
  workspaceAgents,
} from "../src/core/trackerAssignee.js";

const PROJECT_KEY = "dev-1|proj-1";

const workspace = (id, name, over = {}) => ({
  id,
  workspace_id: id,
  name,
  project_id: "proj-1",
  projectKey: PROJECT_KEY,
  entity_id: `run-${id}`,
  ...over,
});

// A board row holds the agent digests for the workspace's conversation. It is
// read the way the inbox reads one: by (project, entity).
const conversation = (workspaceId, agents) => ({
  kind: "branch",
  project_id: "proj-1",
  projectKey: PROJECT_KEY,
  run_id: `run-${workspaceId}`,
  agents,
});

const feed = {
  workspaces: [workspace("ws-1", "wire-facade"), workspace("ws-2", "issues-board")],
  items: [
    conversation("ws-1", [{ id: "agent-1", ordinal: 1, provider: "claude" }, { id: "agent-2", ordinal: 2 }]),
    conversation("ws-2", []),
  ],
};

describe("who this project's agents are", () => {
  it("reads every workspace of the project and the agents standing in it", () => {
    const groups = workspaceAgents(feed, PROJECT_KEY);
    expect(groups.map((group) => [group.name, group.agents.map((agent) => agent.id)])).toEqual([
      ["wire-facade", ["agent-1", "agent-2"]],
      ["issues-board", []],
    ]);
  });

  // An agent has an ordinal and a pattern, not a name; the workspace is what
  // makes one agent tell from another.
  it("names an agent by its workspace and its place on that workspace's strip", () => {
    expect(agentLabels(workspaceAgents(feed, PROJECT_KEY))).toEqual({
      "agent-1": "wire-facade · Agent 1",
      "agent-2": "wire-facade · Agent 2",
    });
  });

  // Both machines mint a `proj-1`; a bare id says nothing about which.
  it("takes only the workspaces of the project it was asked about", () => {
    const otherMachine = { ...workspace("ws-9", "elsewhere"), projectKey: "dev-2|proj-1" };
    expect(workspaceAgents({ ...feed, workspaces: [...feed.workspaces, otherMachine] }, PROJECT_KEY))
      .toHaveLength(2);
  });

  it("has nothing to say about a feed that has not answered yet", () => {
    expect(workspaceAgents(null, PROJECT_KEY)).toEqual([]);
  });
});

describe("the options", () => {
  const options = assigneeOptions(workspaceAgents(feed, PROJECT_KEY));

  it("offers all five kinds, plus unassigning", () => {
    expect(options.map((option) => option.kind)).toEqual([
      "unassign", "user", "project_agent",
      "agent", "agent", "new_agent",
      "new_agent",
      "new_workspace",
    ]);
  });

  it("groups each workspace's agents under that workspace", () => {
    const grouped = options.filter((option) => option.group === "wire-facade").map((option) => option.label);
    expect(grouped).toEqual(["wire-facade · Agent 1", "wire-facade · Agent 2", "New agent in wire-facade"]);
  });

  // A workspace with no agents yet is still a place to start one.
  it("offers a new agent on a workspace that has none", () => {
    expect(options.find((option) => option.id === "new_agent:ws-2").label).toBe("New agent in issues-board");
  });

  // Assigning starts work; the reader should not discover that afterwards.
  it("says what each option is about to do", () => {
    const hintOf = (id) => options.find((option) => option.id === id).hint;
    expect(hintOf("user")).toBe("Nothing is dispatched.");
    expect(hintOf("new_workspace")).toBe("Cuts a workspace in this project and starts an agent on it.");
    expect(hintOf("agent:agent-1")).toBe("Delivers the issue into this agent's conversation.");
  });

  // The harness/model/effort selects appear on exactly the two creating kinds.
  it("names which options open the agent-choice controls", () => {
    expect(options.filter((option) => option.form).map((option) => [option.id, option.form])).toEqual([
      ["new_agent:ws-1", AGENT_FORM],
      ["new_agent:ws-2", AGENT_FORM],
      ["new_workspace", WORKSPACE_FORM],
    ]);
  });

  it("ticks the option an issue's current assignee is", () => {
    expect(selectedOptionId({ kind: "agent", agent_id: "agent-1" })).toBe("agent:agent-1");
    expect(selectedOptionId(null)).toBe("none");
  });

  it("offers the standing three with no workspaces at all", () => {
    expect(assigneeOptions([]).map((option) => option.id)).toEqual(["none", "user", "project_agent", "new_workspace"]);
  });
});

describe("the assignee one option stands for", () => {
  const optionOf = (id) => assigneeOptions(workspaceAgents(feed, PROJECT_KEY)).find((option) => option.id === id);

  it("carries the three standing kinds as the wire takes them", () => {
    expect(assigneeFor(optionOf("none"))).toBeNull();
    expect(assigneeFor(optionOf("user"))).toEqual({ kind: "user" });
    expect(assigneeFor(optionOf("project_agent"))).toEqual({ kind: "project_agent" });
    expect(assigneeFor(optionOf("agent:agent-1"))).toEqual({ kind: "agent", agent_id: "agent-1" });
  });

  it("carries a new agent's workspace", () => {
    expect(assigneeFor(optionOf("new_agent:ws-1"))).toEqual({ kind: "new_agent", workspace_id: "ws-1" });
  });

  // `issues.assign` is a wire verb, and a wire verb takes `provider`; `harness`
  // is the word the MCP tools use for the same field. So the agent-choice
  // controls' answer rides through unchanged.
  it("carries the agent choice under the keys a wire verb takes", () => {
    expect(assigneeFor(optionOf("new_agent:ws-1"), { choice: { provider: "claude", model: "opus", effort: "high" } }))
      .toEqual({ kind: "new_agent", workspace_id: "ws-1", provider: "claude", model: "opus", effort: "high" });
  });

  // agent.add reads a key's presence to tell "run it on this" from "run it on
  // whatever the workspace runs on", so absent is absent and never null.
  it("leaves a choice nobody made off the object rather than sending it null", () => {
    expect(assigneeFor(optionOf("new_agent:ws-1"), { choice: {} })).toEqual({
      kind: "new_agent", workspace_id: "ws-1",
    });
    expect(assigneeFor(optionOf("new_workspace"), { choice: null })).toEqual({ kind: "new_workspace" });
  });

  it("carries a new workspace's name and isolation when they were asked for", () => {
    expect(assigneeFor(optionOf("new_workspace"), { name: " Kanban drag ", isolation: "rift" }))
      .toEqual({ kind: "new_workspace", name: "Kanban drag", isolation: "rift" });
  });

  // Passing no isolation at all is how workspace.create reads "the project's
  // own setting"; saying it any other way would override it.
  it("passes no name and no isolation when none was asked for", () => {
    expect(assigneeFor(optionOf("new_workspace"), { name: "   ", isolation: "" }))
      .toEqual({ kind: "new_workspace" });
  });
});

describe("one press, as issues.assign params", () => {
  const option = { id: "user", kind: "user" };

  it("names the issue and the assignee", () => {
    expect(assignParams("issue-1", option)).toEqual({ issue_id: "issue-1", assignee: { kind: "user" } });
  });

  // The note is delivered under the issue and is not stored on it, so an empty
  // one is left off rather than sent blank.
  it("carries a note when one was written, and nothing when it was not", () => {
    expect(assignParams("issue-1", option, { note: " look at the drag handler " }).note)
      .toBe("look at the drag handler");
    expect(assignParams("issue-1", option, { note: "  " })).not.toHaveProperty("note");
  });

  it("unassigns with a null assignee", () => {
    expect(assignParams("issue-1", { id: "none", kind: "unassign" })).toEqual({
      issue_id: "issue-1", assignee: null,
    });
  });
});
