import { describe, it, expect } from "vitest";
import { coreSourceOf } from "./coreSource.js";
import { gitStatusCells } from "../src/core/gitStatusCells.js";
import {
  AGENT_PATTERN_COUNT,
  AGENT_STARTING,
  agentCanInterrupt,
  agentIsUp,
  agentSessionAnswered,
  agentSessionIsLive,
  agentHeading,
  agentPattern,
  agentWho,
  bubbleTip,
  canRemoveAgent,
  providerLabel,
  railBubbles,
  railEntity,
  railStatusShape,
  railWorkStatus,
  rememberedAgentId,
  removeAgentConfirm,
  selectAgentId,
  startFailuresLearned,
  startupStatusLine,
  elapsedClock,
  workingClock,
  runningClock,
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

describe("what the conversation is called", () => {
  it("wears the topic the agent set, and says it is starting until then", () => {
    expect(agentHeading(agent())).toEqual({ text: "Starting", starting: true });
    expect(agentHeading(agent({ topic: "Unify prompt delivery" }))).toEqual({
      text: "Unify prompt delivery",
      starting: false,
    });
    // A blank topic is no topic: the bridge never sends one, but a client
    // that trusted the field's presence would show an empty header.
    expect(agentHeading(agent({ topic: "   " }))).toEqual({ text: "Starting", starting: true });
    expect(agentHeading(null)).toEqual({ text: "Starting", starting: true });
  });
});

describe("who an agent is", () => {
  it("names the harness", () => {
    expect(providerLabel("claude_adk")).toBe("Claude Code");
    expect(providerLabel("codex_app_server")).toBe("Codex");
    expect(providerLabel("codex")).toBe("Codex TUI");
    // An agent is locked to its harness, so the bubble says which one it is.
    expect(providerLabel("claude")).toBe("Claude Code TUI");
  });

  // The ordinal is the agent's place on the strip — it picks the face the
  // bubble wears, and it is never read out: "Codex TUI 2" named a harness and a
  // number where the human had already named the work.
  it("names an agent by its topic, and by its harness alone until it has one", () => {
    expect(agentWho(agent({ ordinal: 2, provider: "codex", topic: "Fix login redirect" })))
      .toBe("Fix login redirect");
    expect(agentWho(agent({ ordinal: 2, provider: "codex" }))).toBe("Codex TUI");
    expect(agentWho(agent({ ordinal: 3, topic: "   " }))).toBe("Claude Code");
  });

  it("says what an unnamed provider is, rather than nothing", () => {
    expect(providerLabel("")).toBe("Agent");
    expect(providerLabel("gemini")).toBe("gemini");
  });

  it("tells the bubble what it is waiting on", () => {
    const named = { topic: "Fix login redirect" };
    expect(bubbleTip(agent({ ...named, working: true }))).toBe("Fix login redirect — working");
    expect(bubbleTip(agent({ ...named, unread_count: 3, unread_reason: "done" })))
      .toBe("Fix login redirect — The agent finished — review the work");
    expect(bubbleTip(agent(named))).toBe("Fix login redirect");
  });

  // Until the agent names its work there is no topic to hover, and the strip
  // may be two agents on the same harness deep: the tip says the harness so the
  // hover is still worth something, and the painted face tells them apart.
  it("says the harness on a bubble whose agent has not named its work yet", () => {
    expect(bubbleTip(agent())).toBe("Starting · Claude Code");
    expect(bubbleTip(agent({ working: true }))).toBe("Starting · Claude Code — working");
    expect(bubbleTip(agent({ provider: "codex", unread_count: 2, unread_reason: "blocked" })))
      .toBe("Starting · Codex TUI — Blocked — the agent needs you");
  });
});

// The bridge answers a start as soon as the turn is durable and spawns the
// harness behind that answer, so the reply says nothing about a session. The
// row wears `starting` meanwhile, and the entity's own answer is what takes it
// off.
describe("an agent whose start has been asked for", () => {
  it("is up while it is starting, so a message behind the press starts nothing twice", () => {
    expect(agentIsUp(agent({ state: AGENT_STARTING }))).toBe(true);
    expect(agentIsUp(agent({ state: "live" }))).toBe(true);
    expect(agentIsUp(agent({ state: "idle" }))).toBe(false);
    expect(agentIsUp(null)).toBe(false);
  });

  it("is answered only by a live session", () => {
    expect(agentSessionIsLive(agent({ state: "live" }))).toBe(true);
    expect(agentSessionIsLive(agent({ state: AGENT_STARTING }))).toBe(false);
    expect(agentSessionIsLive(agent({ state: "ended" }))).toBe(false);
    expect(agentSessionIsLive(null)).toBe(false);
  });

  // The question a start asks has two answers, and either one ends the wait.
  // A spawn that never came up says nothing about a session, so a client
  // waiting on liveness alone waits out its own grace and then goes quiet.
  it("is also answered by a start that never reached a harness", () => {
    const failed = agent({ state: "idle", start_error: "could not reach the agent: gone" });
    expect(agentSessionAnswered(failed)).toBe(true);
    expect(agentSessionAnswered(agent({ state: "live" }))).toBe(true);
    expect(agentSessionAnswered(agent({ state: "idle" }))).toBe(false);
    expect(bubbleTip(agent({ ...failed, topic: "Fix login redirect" })))
      .toBe("Fix login redirect — could not reach the agent: gone");
  });

  // Said once, and only about a start this rail was already showing an agent
  // for: a reason that was on the payload before the comparison began is what
  // the row already says.
  it("names only the failures that have just arrived", () => {
    const quiet = agent({ id: "ag-1", state: "idle" });
    const failed = agent({ id: "ag-1", state: "idle", start_error: "no worktree" });
    expect(startFailuresLearned([quiet], [failed]).map((a) => a.id)).toEqual(["ag-1"]);
    expect(startFailuresLearned([failed], [failed])).toEqual([]);
    expect(startFailuresLearned([], [failed])).toEqual([]);
    expect(startFailuresLearned([failed], [quiet])).toEqual([]);
  });

  it("says so on its bubble", () => {
    const bubbles = railBubbles({ agents: [agent({ state: AGENT_STARTING })], selectedId: null, kind: "branch" });
    expect(bubbles[0].starting).toBe(true);
    expect(bubbles[0].live).toBe(false);
    expect(bubbleTip(agent({ state: AGENT_STARTING, topic: "Fix login redirect" })))
      .toBe("Fix login redirect — starting…");
    // A bubble with no topic yet already opens on "Starting"; saying it twice
    // tells the hover nothing it did not already have.
    expect(bubbleTip(agent({ state: AGENT_STARTING }))).toBe("Starting · Claude Code");
    expect(railBubbles({ agents: [agent()], selectedId: null, kind: "branch" })[0].starting).toBe(false);
  });

  // Unread wins over starting the way it wins over working: an agent that asked
  // something before its session dropped is still asking.
  it("still says what it is waiting on", () => {
    expect(bubbleTip(agent({ state: AGENT_STARTING, unread_count: 2, unread_reason: "blocked", topic: "Fix login redirect" })))
      .toBe("Fix login redirect — Blocked — the agent needs you");
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

  it("marks no bubble of its own while the chat overview is the selection", () => {
    const bubbles = railBubbles({ agents: [agent()], selectedId: "ag-1", kind: "workspace", selectedKind: "overview" });
    expect(bubbles.map((bubble) => [bubble.type, bubble.active])).toEqual([
      ["agent", false],
      ["add", false],
    ]);
  });

  it("marks the add bubble as the open conversation", () => {
    const bubbles = railBubbles({ agents: [agent()], selectedId: "ag-1", kind: "workspace", selectedKind: "add" });
    expect(bubbles.map((bubble) => [bubble.type, bubble.active])).toEqual([
      ["agent", false],
      ["add", true],
    ]);
  });

  it("shows one ghost where no agent has been born yet, and nothing to add to", () => {
    const bubbles = railBubbles({ agents: [], selectedId: null, kind: "branch" });
    expect(bubbles.map((b) => b.type)).toEqual(["ghost"]);
    expect(bubbles[0].label).toBe("");
    expect(bubbles[0].pattern).toBe(1);
  });
});

// The strip carries the agents the reader watches (#105). One they do not is
// on it only while it is the conversation open, as a temporary bubble marked
// unwatched, and leaving it takes the bubble away.
describe("the strip's watched agents (#105)", () => {
  const agents = () => [
    agent({ id: "ag-1", watched: true }),
    agent({ id: "ag-2", ordinal: 2, watched: false }),
    agent({ id: "ag-3", ordinal: 3 }),
  ];
  const strip = (over) => railBubbles({ agents: agents(), kind: "workspace", ...over })
    .map((bubble) => [bubble.type, bubble.id, !!bubble.unwatched]);

  it("carries only watched agents, and those from a bridge that says nothing about watching", () => {
    expect(strip({ selectedId: "ag-1" })).toEqual([
      ["agent", "ag-1", false],
      ["agent", "ag-3", false],
      ["add", "", false],
    ]);
  });

  it("adds the open unwatched agent in its place, marked and said to be unwatched", () => {
    const bubbles = railBubbles({ agents: agents(), selectedId: "ag-2", kind: "workspace" });
    expect(bubbles.map((bubble) => [bubble.id, !!bubble.unwatched, bubble.active])).toEqual([
      ["ag-1", false, false],
      ["ag-2", true, true],
      ["ag-3", false, false],
      ["", false, false],
    ]);
    expect(bubbles[1].pattern).toBe(2);
    expect(bubbles[1].title).toMatch(/Not watching$/);
  });

  it("drops the temporary bubble once the overview or the + is what is open", () => {
    expect(strip({ selectedId: "ag-2", selectedKind: "overview" }).map(([, id]) => id)).not.toContain("ag-2");
    expect(strip({ selectedId: "ag-2", selectedKind: "add" }).map(([, id]) => id)).not.toContain("ag-2");
  });

  it("keeps the + and no ghost on a work item whose agents are all unwatched", () => {
    const quiet = railBubbles({ agents: [agent({ watched: false })], selectedId: null, selectedKind: "overview", kind: "workspace" });
    expect(quiet.map((bubble) => bubble.type)).toEqual(["add"]);
  });

  it("keeps an unwatched agent under the project's agent off the strip", () => {
    const bubbles = railBubbles({
      agents: [agent({ watched: false }), agent({ id: "ag-2", ordinal: 2, watched: true })],
      selectedId: "ag-project", kind: "workspace",
      projectAgent: { name: "build", entityId: "run-project", agents: [], active: true },
    });
    expect(bubbles.map((bubble) => bubble.id)).toEqual(["run-project", "", "ag-2", ""]);
  });

  it("carries the project's agent only while it is watched or open, marked while unwatched", () => {
    const project = (watched, active) => ({ name: "build", entityId: "run-project",
      agents: [agent({ id: "ag-project", watched })], active });
    const types = (projectAgent) => railBubbles({ agents: [agent()], selectedId: "ag-1", kind: "workspace", projectAgent })
      .map((bubble) => [bubble.type, !!bubble.unwatched]);
    expect(types(project(true, false))).toEqual([["project", false], ["separator", false], ["agent", false], ["add", false]]);
    expect(types(project(false, false))).toEqual([["agent", false], ["add", false]]);
    expect(types(project(false, true))).toEqual([["project", true], ["separator", false], ["agent", false], ["add", false]]);
    expect(railBubbles({ agents: [], kind: "workspace", projectAgent: project(false, true) })[0].title).toMatch(/Not watching$/);
    expect(types({ name: "build", entityId: null, agents: [], active: false })[0]).toEqual(["project", false]);
  });

  it("marks the project's bubble for the open project agent's watch, not the project's", () => {
    const project = (openAgentId, active = true) => ({ name: "build", entityId: "run-project", active, openAgentId,
      agents: [agent({ id: "ag-lead", watched: true }), agent({ id: "ag-helper", ordinal: 2, watched: false })] });
    const [openHelper] = railBubbles({ agents: [agent()], kind: "workspace", projectAgent: project("ag-helper") });
    expect(openHelper.unwatched).toBe(true);
    expect(openHelper.title).toMatch(/Not watching$/);
    expect(railBubbles({ agents: [agent()], kind: "workspace", projectAgent: project("ag-lead") })[0].unwatched).toBe(false);
    expect(railBubbles({ agents: [agent()], kind: "workspace", projectAgent: project(null, false) })[0].unwatched).toBe(false);
  });

  it("opens a watched agent before an unwatched one when nothing is chosen", () => {
    expect(selectAgentId([agent({ watched: false }), agent({ id: "ag-2", ordinal: 2, watched: true })], null)).toBe("ag-2");
    expect(selectAgentId([agent({ watched: false })], null)).toBe("ag-1");
    expect(selectAgentId([agent({ watched: false }), agent({ id: "ag-2", ordinal: 2 })], "ag-1")).toBe("ag-1");
  });

  it("forgets a remembered conversation the reader does not watch", () => {
    const listed = [agent({ watched: false }), agent({ id: "ag-2", ordinal: 2, watched: true })];
    expect(rememberedAgentId(listed, "ag-1")).toBe(null);
    expect(rememberedAgentId(listed, "ag-2")).toBe("ag-2");
    expect(rememberedAgentId([agent()], "ag-1")).toBe("ag-1");
    expect(rememberedAgentId([], "ag-1")).toBe("ag-1");
  });
});

// The project's agent is reachable from every workspace in the project — that
// is what makes it the PROJECT's — so a workspace's strip carries it above a
// line, with the workspace's own agents below.
describe("the project's own page", () => {
  // The page and the strip agree about whose agent this is: on the project's
  // own conversation the bubbles wear what the project's bubble wears above
  // the line on a workspace — the initial, squared off — not a work item's face.
  it("dresses the project's agents in the project's initial", () => {
    const bubbles = railBubbles({ agents: [agent()], selectedId: "ag-1", kind: "project", projectName: "build" });
    expect(bubbles.map((bubble) => bubble.type)).toEqual(["agent"]);
    expect(bubbles[0]).toMatchObject({ label: "B", pattern: null, project: true, active: true });
  });

  it("dresses the ghost the same way before the project's agent is born", () => {
    const [ghost] = railBubbles({ agents: [], kind: "project", projectName: "build" });
    expect(ghost).toMatchObject({ type: "ghost", label: "B", pattern: null, project: true });
  });

  it("leaves a work item's bubbles wearing their faces", () => {
    const [bubble] = railBubbles({ agents: [agent()], kind: "workspace", projectName: "build" });
    expect(bubble.label).toBe("");
    expect(bubble.pattern).not.toBe(null);
    expect(bubble.project).toBeUndefined();
  });
});

describe("the project's agent on a workspace's strip", () => {
  const projectAgent = (over = {}) => ({ name: "build", entityId: null, agents: [], active: false, ...over });

  it("stands above a separator, before the work item's own", () => {
    const bubbles = railBubbles({
      agents: [agent()],
      selectedId: "ag-1",
      kind: "workspace",
      projectAgent: projectAgent(),
    });
    expect(bubbles.map((bubble) => bubble.type)).toEqual(["project", "separator", "agent", "add"]);
  });

  it("wears the project's initial rather than a face of its own", () => {
    const [bubble] = railBubbles({ agents: [agent()], kind: "workspace", projectAgent: projectAgent() });
    expect(bubble.label).toBe("B");
    expect(bubble.pattern).toBe(null);
  });

  // It is there before the project has a conversation at all: an agent nobody
  // has started is still the project's agent, and the bubble says how to start
  // it rather than reading as an empty ghost.
  it("says how to start a project nobody has talked to yet", () => {
    const [bubble] = railBubbles({ agents: [agent()], kind: "workspace", projectAgent: projectAgent() });
    expect(bubble.title).toBe("Project agent for build: send a message to start it");
    expect(bubble.unread).toBe(0);
    expect(bubble.working).toBe(false);
  });

  it("carries the unread and the working of the agents on that conversation", () => {
    const [bubble] = railBubbles({
      agents: [agent()],
      kind: "workspace",
      projectAgent: projectAgent({
        entityId: "run-p",
        agents: [agent({ id: "pa-1", unread_count: 2, working: true })],
      }),
    });
    expect(bubble.unread).toBe(2);
    expect(bubble.working).toBe(true);
    expect(bubble.title).toBe("Project agent for build — 2 unread");
  });

  it("is the open conversation while the panel is on it, and nothing else is", () => {
    const bubbles = railBubbles({
      agents: [agent()],
      selectedId: "ag-1",
      kind: "workspace",
      projectAgent: projectAgent({ entityId: "run-p", active: true }),
    });
    expect(bubbles.map((bubble) => [bubble.type, bubble.active])).toEqual([
      ["project", true],
      ["separator", false],
      ["agent", false],
      ["add", false],
    ]);
  });

  // The half below the line is the work item's, and so is its `+`: the panel
  // being on the project's conversation does not take the workspace's own way
  // of adding an agent off the strip.
  it("keeps the + below the line while the project's conversation is open", () => {
    const bubbles = railBubbles({
      agents: [agent()],
      selectedId: "ag-1",
      kind: "workspace",
      canAdd: true,
      projectAgent: projectAgent({ entityId: "run-p", active: true }),
    });
    expect(bubbles.map((bubble) => bubble.type)).toEqual(["project", "separator", "agent", "add"]);
    expect(bubbles[3].title).toBe("Add another agent to this workspace");
  });

  it("leaves it off where the work item below says nothing can be added to it", () => {
    const bubbles = railBubbles({
      agents: [agent()],
      kind: "workspace",
      canAdd: false,
      projectAgent: projectAgent({ entityId: "run-p", active: true }),
    });
    expect(bubbles.map((bubble) => bubble.type)).toEqual(["project", "separator", "agent"]);
  });

  it("leaves a rail that was not asked for one exactly as it was", () => {
    const bubbles = railBubbles({ agents: [agent()], selectedId: "ag-1", kind: "workspace" });
    expect(bubbles.map((bubble) => bubble.type)).toEqual(["agent", "add"]);
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
    const plan = removeAgentConfirm(agent({ id: "ag-2", ordinal: 2, provider: "codex", topic: "Fix login redirect" }));
    expect(plan.title).toBe('Remove "Fix login redirect" from this branch?');
    expect(plan.actions).toEqual([
      "End the agent's session, if one is running",
      'Remove "Fix login redirect" and its conversation from the branch',
      "Leave the branch and its files untouched",
    ]);
    expect(plan.confirmLabel).toBe("Remove agent");
    expect(plan.danger).toBe(true);
  });

  // Nothing to quote yet: the prompt points at the agent the press came from
  // rather than inventing a name for it.
  it("names an agent that has not set a topic by its harness", () => {
    const plan = removeAgentConfirm(agent({ id: "ag-2", ordinal: 2 }));
    expect(plan.title).toBe("Remove this Claude Code agent from this branch?");
    expect(plan.actions[1]).toBe("Remove this Claude Code agent and its conversation from the branch");
  });

  it("names a workspace when removing one of its agents", () => {
    const plan = removeAgentConfirm(
      agent({ id: "ag-2", ordinal: 2, provider: "codex", topic: "Fix login redirect" }), "workspace");
    expect(plan.title).toBe('Remove "Fix login redirect" from this workspace?');
    expect(plan.actions).toEqual([
      "End the agent's session, if one is running",
      'Remove "Fix login redirect" and its conversation from the workspace',
      "Leave the workspace and its files untouched",
    ]);
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
      { kind: "branch", project_id: "p1", branch: "main", worktree_id: "wt-9", agents: [] },
      "branch",
    );
    expect(entity).toMatchObject({
      entityId: "wt-9",
      adoptable: true,
      worktreeId: "wt-9",
      canAdd: false,
    });
    expect(entity.thread).toBe(null);
  });

  // A project is a template: there is no checkout under it to adopt, and its
  // one agent works in a scratch directory Build owns. So the rail is a
  // conversation and nothing else — no adoption, no second agent, no
  // directories for a path an agent writes to be resolved against.
  it("reads a project: its conversation owner, and nothing to adopt", () => {
    const entity = railEntity(
      { project_id: "p1", entity_id: "run-7", run_id: "run-7", agents: [agent()], thread: { items: [] } },
      "project",
    );
    expect(entity).toMatchObject({
      entityId: "run-7",
      kind: "project",
      projectId: "p1",
      branch: null,
      worktreeId: null,
      adoptable: false,
      canAdd: false,
      chatCapable: true,
    });
    expect(entity.directories).toEqual([]);
    expect(entity.thread.items).toEqual([]);
    expect(railEntity(null, "project")).toMatchObject({ entityId: null, kind: "project", agents: [], thread: null });
  });

  // One agent, so no `+`: adding another is a branch's and a workspace's verb.
  it("offers no second agent on a project", () => {
    const bubbles = railBubbles({ agents: [agent()], selectedId: "ag-1", kind: "project" });
    expect(bubbles.map((bubble) => bubble.type)).toEqual(["agent"]);
    expect(canRemoveAgent({ agents: [agent()], agentId: "ag-1", kind: "project" })).toBe(false);
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

  it("runs the status clock as M:SS, and as H:MM from ninety minutes on", () => {
    expect(runningClock(42)).toBe("0:42");
    expect(runningClock(60)).toBe("1:00");
    expect(runningClock(750)).toBe("12:30");
    expect(runningClock(3600)).toBe("60:00");
    expect(runningClock(5399)).toBe("89:59");
    expect(runningClock(5400)).toBe("1:30");
    expect(runningClock(90000)).toBe("25:00");
  });

  it("clocks how long ago a stamp was, in the running clock's own form", () => {
    expect(elapsedClock(NOW - 65_000, NOW)).toBe("1:05");
    expect(elapsedClock(NOW, NOW)).toBe("0:00");
    expect(elapsedClock(NOW + 5_000, NOW)).toBe("0:00");
  });

  it("reads the timer from the selected agent and git facts from the row", () => {
    const row = { working_time: { since: ago(750), seconds: 750 }, stat: { insertions: 42, deletions: 7, ahead: 2, behind: 0 } };
    const status = railWorkStatus(row, NOW, [], "Agent", agent({ working_time: { since: ago(750), seconds: 750 } }));
    expect(status.working).toBe("12:30");
    expect(status.starting).toBe("");
    expect(status.git).toEqual(gitStatusCells({ insertions: 42, deletions: 7, ahead: 2, behind: 0 }));
    expect(railWorkStatus({ working_time: null, stat: null }, NOW)).toEqual({ working: "", starting: "", git: [] });
    expect(railWorkStatus(null, NOW)).toEqual({ working: "", starting: "", git: [] });
  });

  it("never borrows the entity aggregate for an agent", () => {
    const row = { working_time: { since: ago(750), seconds: 750 } };
    expect(railWorkStatus(row, NOW, [], "Agent", agent({ working_time: null })).working).toBe("");
    expect(railWorkStatus(row, NOW, [], "Agent", agent({ working_time: { seconds: 5 } })).working).toBe("0:05");
  });

  it("names the lead's shape, the working clock winning over the startup line", () => {
    expect(railStatusShape({ working: "5s", starting: "Run started · 2m ago" })).toBe("working");
    expect(railStatusShape({ working: "", starting: "Run started · 2m ago" })).toBe("starting");
    expect(railStatusShape({ working: "", starting: "" })).toBe("quiet");
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
      git: [],
    });
    expect(railWorkStatus({}, NOW, items, "Agent", agent({ working_time: { since: ago(5), seconds: 5 } }))).toEqual({
      working: "0:05",
      starting: "",
      git: [],
    });
  });

  it("names the session's start in the harness's own words", () => {
    const items = [startup("session_started", 120)];
    expect(startupStatusLine(items, "Codex")).toEqual({ title: "Codex session started", at: NOW - 120000 });
    expect(railWorkStatus(null, NOW, items, "Codex").starting).toBe("Codex session started · 2m ago");
  });

  it("says the title alone when the event carried no stamp", () => {
    const items = [{ type: "event", data: { event: "run_started" } }];
    expect(railWorkStatus(null, NOW, items).starting).toBe("Run started");
  });
});

describe("what the rail's model is allowed to reach for", () => {
  it("stays a pure model: no DOM, no wire, no renderer", () => {
    const source = coreSourceOf("agentRailModel.js");
    expect(source).not.toContain("document");
    expect(source).not.toContain("App.call");
    expect(source).not.toContain("fetch(");
    expect(source).not.toContain('from "./thread.js"');
  });
});
