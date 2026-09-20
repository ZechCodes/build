// What a workspace's agents are carrying: the same grouping as an agent's own
// conversation entry, asked of each agent in the workspace.

import { describe, expect, it } from "vitest";
import {
  agentLabel,
  workspaceAgentIds,
  workspaceIssueGroups,
  workspaceOpenIssueCount,
} from "../src/core/trackerWorkspaceIssues.js";
import { issue } from "./trackerWireFixture.js";

const ONE = "agent-01M2ONE";
const TWO = "agent-01M2TWO";
const ELSEWHERE = "agent-01M2ELSE";

const agents = [{ id: ONE, ordinal: 1 }, { id: TWO, ordinal: 2 }];
const held = (agentId, over = {}) => issue({ assignee: { kind: "agent", agent_id: agentId }, ...over });

const sections = (issues, list = agents) => workspaceIssueGroups(issues, list);
const sectionFor = (issues, agentId) => sections(issues).find((one) => one.agentId === agentId);

describe("the workspace's agents", () => {
  it("are the ids its row carries", () => {
    expect(workspaceAgentIds(agents)).toEqual([ONE, TWO]);
  });

  // A workspace nobody has spoken in has no row and no agents, which is not an
  // error — it holds no issues for the same reason.
  it("are none for a workspace with no conversation yet", () => {
    expect(workspaceAgentIds(null)).toEqual([]);
    expect(workspaceAgentIds([{ ordinal: 1 }])).toEqual([]);
  });

  // An agent has no name of its own, only a place on the strip — the same
  // place the rail's bubbles read across.
  it("are named by their place on the strip", () => {
    expect(agentLabel({ id: ONE, ordinal: 2 }, 0)).toBe("Agent 2");
    expect(agentLabel({ id: ONE }, 1)).toBe("Agent 2");
  });
});

describe("grouping by agent", () => {
  it("gives each agent the same groups its own conversation shows", () => {
    const issues = [
      held(ONE, { number: 1, status: "in_progress" }),
      held(ONE, { number: 2, status: "ready" }),
      held(ONE, { number: 3, status: "done" }),
    ];
    expect(sectionFor(issues, ONE).groups.map((one) => one.id)).toEqual(["working", "holding", "finished"]);
  });

  it("keeps the agents in the order the row lists them", () => {
    const issues = [held(TWO, { number: 1, status: "in_progress" }), held(ONE, { number: 2, status: "in_progress" })];
    expect(sections(issues).map((one) => one.label)).toEqual(["Agent 1", "Agent 2"]);
  });

  // The view answers "what is being worked here"; an agent with nothing to
  // show is not part of that answer.
  it("leaves out an agent holding nothing", () => {
    const issues = [held(ONE, { number: 1, status: "in_progress" })];
    expect(sections(issues).map((one) => one.agentId)).toEqual([ONE]);
  });

  it("leaves out an issue held by an agent of another workspace", () => {
    expect(sections([held(ELSEWHERE, { number: 9, status: "in_progress" })])).toEqual([]);
  });

  // An issue somebody else holds is not this workspace's work, however many
  // of its agents are following it. Tracking belongs in a conversation.
  it("never shows what an agent merely tracks", () => {
    const issues = [issue({ number: 9, assignee: { kind: "agent", agent_id: ELSEWHERE }, trackers: [ONE] })];
    expect(sections(issues)).toEqual([]);
  });

  it("has nothing to say about a workspace with no agents", () => {
    expect(sections([held(ONE, { number: 1 })], [])).toEqual([]);
    expect(workspaceIssueGroups(null, agents)).toEqual([]);
  });
});

describe("the badge's count", () => {
  const issues = [
    held(ONE, { number: 1, status: "in_progress" }),
    held(ONE, { number: 2, status: "ready" }),
    held(TWO, { number: 3, status: "in_review" }),
    held(TWO, { number: 4, status: "done" }),
    held(ONE, { number: 5, state: "closed" }),
    held(ELSEWHERE, { number: 6, status: "in_progress" }),
  ];

  // A badge is a call to look, and finished work is not one.
  it("counts what is open across the workspace's agents, and nothing finished", () => {
    expect(workspaceOpenIssueCount(issues, agents)).toBe(3);
  });

  it("counts nothing when the agents hold nothing open", () => {
    expect(workspaceOpenIssueCount([held(ONE, { number: 1, status: "done" })], agents)).toBe(0);
    expect(workspaceOpenIssueCount([], agents)).toBe(0);
    expect(workspaceOpenIssueCount(issues, [])).toBe(0);
  });
});
