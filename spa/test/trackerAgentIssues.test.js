// One agent's issues, grouped the way a reader of its conversation wants them:
// what it is working on, what it is holding, what it has finished, what it is
// only watching.

import { describe, expect, it } from "vitest";
import {
  FINISHED_GROUP,
  HOLDING_GROUP,
  WATCHING_GROUP,
  WORKING_GROUP,
  agentIssueCount,
  agentIssueGroups,
  agentOpenIssueCount,
  assignedTo,
  groupOf,
  isFinished,
  tracks,
} from "../src/core/trackerAgentIssues.js";
import { issue } from "./trackerWireFixture.js";

const ME = "agent-01M2ME";
const THEM = "agent-01M2THEM";

/** An issue this agent holds, in whichever column the case is about. */
const mine = (over = {}) => issue({ assignee: { kind: "agent", agent_id: ME }, ...over });

const groupIds = (issues, agentId = ME) => agentIssueGroups(issues, agentId).map((one) => one.id);
const inGroup = (issues, id, agentId = ME) =>
  (agentIssueGroups(issues, agentId).find((one) => one.id === id)?.issues || []).map((one) => one.number);

describe("who an issue belongs to", () => {
  it("is assigned to an agent only when the tagged shape names that agent", () => {
    expect(assignedTo(mine(), ME)).toBe(true);
    expect(assignedTo(mine(), THEM)).toBe(false);
    expect(assignedTo(issue({ assignee: null }), ME)).toBe(false);
    // A project agent and the user are assignees, but neither is an agent of a
    // workspace, so neither can be this one.
    expect(assignedTo(issue({ assignee: { kind: "project_agent" } }), ME)).toBe(false);
    expect(assignedTo(issue({ assignee: { kind: "user" } }), ME)).toBe(false);
  });

  it("is tracked when the trackers list names the agent", () => {
    expect(tracks(issue({ trackers: [THEM, ME] }), ME)).toBe(true);
    expect(tracks(issue({ trackers: [THEM] }), ME)).toBe(false);
  });

  // #13 adds `trackers`. A bridge that predates it sends no such field, and
  // every issue then reads as untracked — which is the right answer there, and
  // means the Tracking group simply never appears.
  it("reads an issue with no trackers field as tracked by nobody", () => {
    expect(tracks(issue(), ME)).toBe(false);
    expect(groupIds([issue({ assignee: null })])).toEqual([]);
  });
});

describe("what counts as over", () => {
  // Closing and the Done column are independent, and either one makes the
  // issue history to a reader asking what an agent is doing now.
  it("is closed anywhere, or open in Done", () => {
    expect(isFinished(issue({ state: "closed", status: "backlog" }))).toBe(true);
    expect(isFinished(issue({ state: "open", status: "done" }))).toBe(true);
    expect(isFinished(issue({ state: "open", status: "in_review" }))).toBe(false);
  });
});

describe("the four groups", () => {
  it("puts in-progress work first and the rest of what it holds below", () => {
    const issues = [
      mine({ number: 1, status: "backlog" }),
      mine({ number: 2, status: "in_progress" }),
      mine({ number: 3, status: "in_review" }),
      mine({ number: 4, status: "ready" }),
    ];
    expect(groupIds(issues)).toEqual([WORKING_GROUP, HOLDING_GROUP]);
    expect(inGroup(issues, WORKING_GROUP)).toEqual([2]);
    expect(inGroup(issues, HOLDING_GROUP)).toEqual([4, 3, 1]);
  });

  it("collects what is finished, whether it was closed or moved to Done", () => {
    const issues = [
      mine({ number: 1, state: "closed", status: "in_progress" }),
      mine({ number: 2, status: "done" }),
      mine({ number: 3, status: "in_progress" }),
    ];
    expect(inGroup(issues, FINISHED_GROUP)).toEqual([2, 1]);
    expect(inGroup(issues, WORKING_GROUP)).toEqual([3]);
  });

  it("watches what it tracks and does not hold", () => {
    const issues = [issue({ number: 9, assignee: { kind: "agent", agent_id: THEM }, trackers: [ME] })];
    expect(groupIds(issues)).toEqual([WATCHING_GROUP]);
    expect(inGroup(issues, WATCHING_GROUP)).toEqual([9]);
  });

  // The assignee is tracked automatically (#13), so almost every assigned issue
  // is also a tracked one. Counting it twice would double every row.
  it("never lists an issue twice when it is both assigned and tracked", () => {
    const issues = [mine({ number: 5, status: "in_progress", trackers: [ME] })];
    expect(groupIds(issues)).toEqual([WORKING_GROUP]);
    expect(inGroup(issues, WATCHING_GROUP)).toEqual([]);
  });

  // A category a reader has none of is not worth a heading saying so.
  it("leaves an empty group out rather than drawing it empty", () => {
    expect(groupIds([mine({ number: 1, status: "in_progress" })])).toEqual([WORKING_GROUP]);
    expect(groupIds([])).toEqual([]);
    expect(groupIds(null)).toEqual([]);
  });

  it("collapses the two that are not news, and only those", () => {
    const issues = [
      mine({ number: 1, status: "in_progress" }),
      mine({ number: 2, status: "ready" }),
      mine({ number: 3, status: "done" }),
      issue({ number: 4, assignee: { kind: "agent", agent_id: THEM }, trackers: [ME] }),
    ];
    expect(agentIssueGroups(issues, ME).map((one) => [one.id, one.collapses])).toEqual([
      [WORKING_GROUP, false],
      [HOLDING_GROUP, false],
      [FINISHED_GROUP, true],
      [WATCHING_GROUP, true],
    ]);
  });

  it("ignores an issue that is neither assigned to nor tracked by this agent", () => {
    expect(groupIds([issue({ number: 1, assignee: { kind: "agent", agent_id: THEM } })])).toEqual([]);
  });

  it("answers nothing for no agent at all", () => {
    expect(agentIssueGroups([mine({ number: 1 })], null)).toEqual([]);
    expect(groupOf(mine({ number: 1 }), "")).toBeNull();
  });
});

describe("the order within a group", () => {
  // An activity entry answers "what has moved", not "what was filed" — which
  // is why each card wears the time it moved. The Issues tab's newest-first
  // order is a catalogue's, and is deliberately not this.
  it("puts what moved most recently first", () => {
    const issues = [
      mine({ number: 1, status: "ready", updated_at: "2026-08-21T10:00:00Z" }),
      mine({ number: 2, status: "ready", updated_at: "2026-08-23T10:00:00Z" }),
      mine({ number: 3, status: "ready", updated_at: "2026-08-22T10:00:00Z" }),
    ];
    expect(inGroup(issues, HOLDING_GROUP)).toEqual([2, 3, 1]);
  });

  it("breaks a tie by number, so every repaint agrees", () => {
    const same = "2026-08-21T10:00:00Z";
    const issues = [
      mine({ number: 7, status: "ready", updated_at: same }),
      mine({ number: 9, status: "ready", updated_at: same }),
      mine({ number: 8, status: "ready", updated_at: same }),
    ];
    expect(inGroup(issues, HOLDING_GROUP)).toEqual([9, 8, 7]);
  });

  // 0 is "unknown", not "now": an issue with no readable stamp sorts oldest
  // rather than jumping to the top of the entry.
  it("sorts an unreadable stamp oldest rather than newest", () => {
    const issues = [
      mine({ number: 1, status: "ready", updated_at: "not a date" }),
      mine({ number: 2, status: "ready", updated_at: "2026-08-21T10:00:00Z" }),
    ];
    expect(inGroup(issues, HOLDING_GROUP)).toEqual([2, 1]);
  });
});

describe("the counts", () => {
  const issues = [
    mine({ number: 1, status: "in_progress" }),
    mine({ number: 2, status: "ready" }),
    mine({ number: 3, status: "done" }),
    issue({ number: 4, assignee: { kind: "agent", agent_id: THEM }, trackers: [ME] }),
    issue({ number: 5, assignee: { kind: "agent", agent_id: THEM } }),
  ];

  it("counts everything this agent has anything to do with", () => {
    expect(agentIssueCount(issues, ME)).toBe(4);
  });

  // What a badge asks: is this agent carrying anything right now.
  it("counts only what it is holding and not finished", () => {
    expect(agentOpenIssueCount(issues, ME)).toBe(2);
  });

  it("counts nothing for an agent with nothing", () => {
    expect(agentIssueCount(issues, "agent-nobody")).toBe(0);
    expect(agentOpenIssueCount([], ME)).toBe(0);
  });
});
