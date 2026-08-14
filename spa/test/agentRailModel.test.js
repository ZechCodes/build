import { describe, it, expect } from "vitest";
import {
  agentTitle,
  bubbleTip,
  completionReportSections,
  providerLabel,
  railBubbles,
  railEntity,
  selectAgentId,
} from "../src/core/agentRailModel.js";

const agent = (over = {}) => ({
  id: "ag-1",
  ordinal: 1,
  provider: "claude",
  state: "idle",
  unread_count: 0,
  unread_reason: null,
  working: false,
  ...over,
});

describe("who an agent is", () => {
  it("names the harness and which one of it this is", () => {
    expect(providerLabel("claude")).toBe("Claude Code");
    expect(providerLabel("codex")).toBe("Codex");
    expect(agentTitle(agent({ ordinal: 2, provider: "codex" }))).toBe("Codex 2");
  });

  it("says what an unnamed provider is, rather than nothing", () => {
    expect(providerLabel("")).toBe("Agent");
    expect(providerLabel("gemini")).toBe("gemini");
  });

  it("tells the bubble what it is waiting on", () => {
    expect(bubbleTip(agent({ working: true }))).toBe("Claude Code 1 — working");
    expect(bubbleTip(agent({ unread_count: 3, unread_reason: "done" })))
      .toBe("Claude Code 1 — The agent finished — review the work");
    expect(bubbleTip(agent())).toBe("Claude Code 1");
  });
});

describe("the bubble strip", () => {
  it("is one bubble per agent, marking the expanded one", () => {
    const bubbles = railBubbles({
      agents: [agent(), agent({ id: "ag-2", ordinal: 2, unread_count: 2, working: true })],
      selectedId: "ag-2",
      kind: "branch",
    });
    expect(bubbles.map((b) => [b.type, b.id, b.active])).toEqual([
      ["agent", "ag-1", false],
      ["agent", "ag-2", true],
      ["add", "", false],
    ]);
    expect(bubbles[1].unread).toBe(2);
    expect(bubbles[1].working).toBe(true);
  });

  it("offers a second agent on a branch and never on an issue", () => {
    const types = (kind) => railBubbles({ agents: [agent()], selectedId: null, kind }).map((b) => b.type);
    expect(types("branch")).toContain("add");
    expect(types("issue")).not.toContain("add");
  });

  it("shows one ghost where no agent has been born yet, and nothing to add to", () => {
    const bubbles = railBubbles({ agents: [], selectedId: null, kind: "branch" });
    expect(bubbles.map((b) => b.type)).toEqual(["ghost"]);
    expect(bubbles[0].label).toBe("1");
  });
});

describe("which conversation is open", () => {
  it("keeps the chosen agent while it exists, else opens the first", () => {
    const agents = [agent(), agent({ id: "ag-2", ordinal: 2 })];
    expect(selectAgentId(agents, "ag-2")).toBe("ag-2");
    expect(selectAgentId(agents, "gone")).toBe("ag-1");
    expect(selectAgentId(agents, null)).toBe("ag-1");
    expect(selectAgentId([], "ag-1")).toBe(null);
  });
});

describe("what the rail is the rail of", () => {
  it("reads a branch row: the entity it takes attention as, and its agents", () => {
    const entity = railEntity(
      {
        kind: "branch",
        project_id: "p1",
        branch: "build/x",
        run_id: "run-7",
        worktree_id: "wt-3",
        agents: [agent()],
        run: { thread: { items: [{ type: "message" }] } },
      },
      "branch",
    );
    expect(entity).toMatchObject({
      entityId: "run-7",
      kind: "branch",
      projectId: "p1",
      branch: "build/x",
      adoptable: false,
      canAdd: true,
    });
    expect(entity.agents).toHaveLength(1);
    expect(entity.thread.items).toHaveLength(1);
  });

  it("reads a checkout Build owns nothing in as one to adopt on the first message", () => {
    const entity = railEntity(
      { kind: "branch", project_id: "p1", branch: "main", worktree_id: "wt-9", primary: true, agents: [] },
      "branch",
    );
    expect(entity).toMatchObject({
      entityId: "wt-9",
      adoptable: true,
      primary: true,
      worktreeId: "wt-9",
      canAdd: false,
    });
    expect(entity.thread).toBe(null);
  });

  it("reads an issue: its own id, its one agent, its own conversation", () => {
    const entity = railEntity(
      { issue_id: "plan-2", project_id: "p1", agents: [agent()], thread: { items: [] } },
      "issue",
    );
    expect(entity).toMatchObject({ entityId: "plan-2", kind: "issue", adoptable: false, canAdd: false });
    expect(entity.thread.items).toEqual([]);
  });

  it("has nothing to say about a payload that never arrived", () => {
    expect(railEntity(null, "branch")).toMatchObject({ entityId: null, agents: [], thread: null });
  });
});

describe("the completion report", () => {
  it("keeps the lists that were filled in, in reading order", () => {
    const sections = completionReportSections({
      critical_files: ["src/a.rs — holds the change"],
      risk_notes: [],
      decisions: ["kept the old name"],
      skips: ["did not touch the migration"],
    });
    expect(sections.map((s) => s.title)).toEqual(["Critical files", "Decisions", "Skipped"]);
    expect(sections[0].items).toEqual(["src/a.rs — holds the change"]);
  });

  it("is nothing at all when the agent filled in nothing", () => {
    expect(completionReportSections({ critical_files: [], risk_notes: [] })).toEqual([]);
    expect(completionReportSections(null)).toEqual([]);
  });
});
