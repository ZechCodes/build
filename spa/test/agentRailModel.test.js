import { describe, it, expect } from "vitest";
import {
  AGENT_PATTERN_COUNT,
  agentCanInterrupt,
  agentPattern,
  agentTitle,
  aheadBehindText,
  bubbleTip,
  canRemoveAgent,
  completionReportSections,
  providerLabel,
  railBubbles,
  railEntity,
  railWorkStatus,
  removeAgentConfirm,
  selectAgentId,
  startupStatusLine,
  statText,
  workingClock,
  workingSeconds,
} from "../src/core/agentRailModel.js";

const NOW = Date.parse("2026-08-13T12:00:00Z");
const ago = (seconds) => new Date(NOW - seconds * 1000).toISOString();

const agent = (over = {}) => ({
  id: "ag-1",
  ordinal: 1,
  provider: "claude_adk",
  state: "idle",
  unread_count: 0,
  unread_reason: null,
  working: false,
  ...over,
});

describe("who an agent is", () => {
  it("names the harness and which one of it this is", () => {
    expect(providerLabel("claude_adk")).toBe("Claude Code");
    expect(providerLabel("codex")).toBe("Codex");
    // An agent is locked to its harness, so the two claude harnesses are two
    // agents and the bubble says which one it is.
    expect(providerLabel("claude")).toBe("Claude Code TUI");
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

describe("whether the turn in flight can be stopped", () => {
  it("is yes only for an agent that is working and announced the interrupt", () => {
    expect(agentCanInterrupt(agent({ working: true, can_interrupt: true }))).toBe(true);
  });

  it("is no when there is no turn to stop", () => {
    expect(agentCanInterrupt(agent({ working: false, can_interrupt: true }))).toBe(false);
  });

  // Unlike the terminal — which every carrier had before the question could be
  // asked — an interrupt is a capability the child announces at startup. So a
  // bridge that never mentions it, and a child that never announced it, both
  // mean the same thing: the plain Send stands.
  it("is no when the digest does not mention it", () => {
    expect(agentCanInterrupt(agent({ working: true }))).toBe(false);
    expect(agentCanInterrupt(agent({ working: true, can_interrupt: false }))).toBe(false);
    expect(agentCanInterrupt(agent({ working: true, can_interrupt: "yes" }))).toBe(false);
    expect(agentCanInterrupt(null)).toBe(false);
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
    expect(bubbles[0].label).toBe("");
    expect(bubbles[0].pattern).toBe(1);
  });
});

// An agent is not a number. Each wears one of a handful of patterns instead —
// animated while it is working, frozen where it stopped when it is not — so the
// strip reads as a set of faces rather than a numbered list.
describe("the pattern an agent wears", () => {
  it("gives each of the first agents on a work item a pattern of its own", () => {
    const patterns = [1, 2, 3, 4, 5].map((ordinal) => agentPattern(ordinal));
    expect(new Set(patterns).size).toBe(AGENT_PATTERN_COUNT);
    expect(patterns.every((pattern) => pattern >= 1 && pattern <= AGENT_PATTERN_COUNT)).toBe(true);
  });

  it("is the same pattern every time for the same agent, and wraps past the last", () => {
    expect(agentPattern(3)).toBe(agentPattern(3));
    expect(agentPattern(AGENT_PATTERN_COUNT + 1)).toBe(agentPattern(1));
  });

  it("dresses an agent with no ordinal as the first one", () => {
    expect(agentPattern(0)).toBe(agentPattern(1));
    expect(agentPattern(undefined)).toBe(agentPattern(1));
  });

  it("puts the pattern on the bubble, and no number anywhere on it", () => {
    const bubbles = railBubbles({
      agents: [agent(), agent({ id: "ag-2", ordinal: 2 })],
      selectedId: "ag-1",
      kind: "branch",
    });
    expect(bubbles.slice(0, 2).map((bubble) => bubble.pattern)).toEqual([agentPattern(1), agentPattern(2)]);
    expect(bubbles.slice(0, 2).map((bubble) => bubble.label)).toEqual(["", ""]);
    // The `+` is a control, not an agent: it keeps its glyph and wears no pattern.
    expect(bubbles[2].label).toBe("+");
    expect(bubbles[2].pattern).toBe(null);
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

describe("which agent can be taken back off", () => {
  const agents = [agent(), agent({ id: "ag-2", ordinal: 2 }), agent({ id: "ag-3", ordinal: 3 })];

  it("offers removal for every agent on a branch, the first and the last included", () => {
    expect(canRemoveAgent({ agents, agentId: "ag-1", kind: "branch" })).toBe(true);
    expect(canRemoveAgent({ agents, agentId: "ag-2", kind: "branch" })).toBe(true);
    expect(canRemoveAgent({ agents, agentId: "ag-3", kind: "branch" })).toBe(true);
    // A branch with no agent at all is a working branch: its chat tab asks
    // which agent to start one on.
    expect(canRemoveAgent({ agents: [agent()], agentId: "ag-1", kind: "branch" })).toBe(true);
  });

  it("offers nothing on an issue, whose one agent is the issue's own conversation", () => {
    expect(canRemoveAgent({ agents, agentId: "ag-2", kind: "issue" })).toBe(false);
  });

  it("offers nothing for an agent that is not on this work item, or none at all", () => {
    expect(canRemoveAgent({ agents, agentId: "ag-gone", kind: "branch" })).toBe(false);
    expect(canRemoveAgent({ agents, agentId: null, kind: "branch" })).toBe(false);
    expect(canRemoveAgent({ agents: [], agentId: "ag-1", kind: "branch" })).toBe(false);
    expect(canRemoveAgent()).toBe(false);
  });

  it("outlines what removal actually does before it is confirmed", () => {
    const plan = removeAgentConfirm(agent({ id: "ag-2", ordinal: 2, provider: "codex" }));
    expect(plan.title).toBe("Remove Codex 2 from this branch?");
    expect(plan.actions).toEqual([
      "End the agent's session, if one is running",
      "Remove Codex 2 and its conversation from the branch",
      "Leave the branch and its files untouched",
    ]);
    expect(plan.confirmLabel).toBe("Remove agent");
    expect(plan.danger).toBe(true);
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

describe("the pinned status line above the composer", () => {
  it("tickers the working time off the stamp, and falls back to the count", () => {
    expect(workingSeconds({ since: ago(120), seconds: 5 }, NOW)).toBe(120);
    expect(workingSeconds({ since: "not a time", seconds: 5 }, NOW)).toBe(5);
    expect(workingSeconds(null, NOW)).toBeNull();
  });

  it("clocks seconds alone, minutes and seconds, then hours and minutes — never days", () => {
    expect(workingClock(42)).toBe("42s");
    expect(workingClock(60)).toBe("1m 00s");
    expect(workingClock(750)).toBe("12m 30s");
    expect(workingClock(3600)).toBe("1h 00m");
    expect(workingClock(90000)).toBe("25h 00m");
  });

  it("says the additions and deletions, and nothing when there are none", () => {
    expect(statText({ insertions: 42, deletions: 7 })).toBe("+42 −7");
    expect(statText({ insertions: 0, deletions: 0, files_changed: 0 })).toBe("");
    expect(statText("+4 −1")).toBe("+4 −1");
    expect(statText(null)).toBe("");
  });

  it("says how far the branch stands from upstream, only for the counts that are nonzero", () => {
    expect(aheadBehindText({ ahead: 2, behind: 1 })).toBe("↑2 ↓1");
    expect(aheadBehindText({ ahead: 2, behind: 0 })).toBe("↑2");
    expect(aheadBehindText({ ahead: 0, behind: 0 })).toBe("");
    expect(aheadBehindText(null)).toBe("");
  });

  it("reads all three off the row, each blank when the row does not know it", () => {
    const row = { working_time: { since: ago(750), seconds: 750 }, stat: { insertions: 42, deletions: 7, ahead: 2, behind: 0 } };
    expect(railWorkStatus(row, NOW)).toEqual({ working: "12m 30s", starting: "", sync: "↑2", stat: "+42 −7" });
    expect(railWorkStatus({ working_time: null, stat: null }, NOW)).toEqual({ working: "", starting: "", sync: "", stat: "" });
    expect(railWorkStatus(null, NOW)).toEqual({ working: "", starting: "", sync: "", stat: "" });
  });
});

describe("the startup line the status slot borrows from the conversation", () => {
  const startup = (event, secondsAgo) => ({ type: "event", data: { event, created_at: ago(secondsAgo) } });

  it("reads the newest item when that item is a startup event", () => {
    expect(startupStatusLine([startup("run_started", 120)])).toEqual({ title: "Run started", at: NOW - 120000 });
    expect(startupStatusLine([startup("run_started", 900), startup("session_started", 120)]))
      .toEqual({ title: "Agent session started", at: NOW - 120000 });
  });

  it("is nothing once anything newer is on the record", () => {
    expect(startupStatusLine([])).toBeNull();
    expect(startupStatusLine([startup("session_started", 120), { type: "message", data: { role: "agent", body: "on it" } }])).toBeNull();
    expect(startupStatusLine([startup("session_started", 120), startup("reasoning", 60)])).toBeNull();
    expect(startupStatusLine([{ type: "event", data: { event: "done" } }])).toBeNull();
  });

  it("carries no stamp for an event that arrived without one", () => {
    expect(startupStatusLine([{ type: "event", data: { event: "run_started" } }])).toEqual({ title: "Run started", at: null });
  });

  it("stands in the working slot with its age, and only while nothing is working", () => {
    const items = [startup("session_started", 120)];
    expect(railWorkStatus({ working_time: null, stat: null }, NOW, items)).toEqual({
      working: "",
      starting: "Agent session started · 2m ago",
      sync: "",
      stat: "",
    });
    expect(railWorkStatus({ working_time: { since: ago(5), seconds: 5 } }, NOW, items)).toEqual({
      working: "5s",
      starting: "",
      sync: "",
      stat: "",
    });
  });

  it("says the title alone when the event carried no stamp", () => {
    const items = [{ type: "event", data: { event: "run_started" } }];
    expect(railWorkStatus(null, NOW, items).starting).toBe("Run started");
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
