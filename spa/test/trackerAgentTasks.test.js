// One agent's tasks: who a task belongs to, where it stands for that agent,
// and the ordered rows its Tasks surface is drawn from (#34).
//
// The four-group shape this file used to check went with the inline block the
// surfaces pill replaced; the questions behind it did not change, and the
// entries below are the same four answers in the shape the surfaces layer
// draws.

import { describe, expect, it } from "vitest";
import {
  agentTaskEntries,
  agentOpenTaskCount,
  assignedTo,
  groupOf,
  isFinished,
  tracks,
} from "../src/core/trackerAgentTasks.js";
import { task } from "./trackerWireFixture.js";

const ME = "agent-01M2ME";
const THEM = "agent-01M2THEM";

/** A task this agent holds, in whichever column the case is about. */
const mine = (over = {}) => task({ assignee: { kind: "agent", agent_id: ME }, ...over });

/** The surface rows, said as `state:number`, which is the order and the
 *  standing in one string. */
const entries = (tasks, agentId = ME) =>
  agentTaskEntries(tasks, agentId).map((one) => `${one.state}:${one.number}`);
const numbersIn = (tasks, state, agentId = ME) =>
  agentTaskEntries(tasks, agentId).filter((one) => one.state === state).map((one) => one.number);

describe("who a task belongs to", () => {
  it("is assigned to an agent only when the tagged shape names that agent", () => {
    expect(assignedTo(mine(), ME)).toBe(true);
    expect(assignedTo(mine(), THEM)).toBe(false);
    expect(assignedTo(task({ assignee: null }), ME)).toBe(false);
    // A project agent and the user are assignees, but neither is an agent of a
    // workspace, so neither can be this one.
    expect(assignedTo(task({ assignee: { kind: "project_agent" } }), ME)).toBe(false);
    expect(assignedTo(task({ assignee: { kind: "user" } }), ME)).toBe(false);
  });

  it("is tracked when the trackers list names the agent", () => {
    expect(tracks(task({ trackers: [THEM, ME] }), ME)).toBe(true);
    expect(tracks(task({ trackers: [THEM] }), ME)).toBe(false);
  });

  // #13 adds `trackers`. A bridge that predates it sends no such field, and
  // every task then reads as untracked — which is the right answer there, and
  // means the Tracking group simply never appears.
  it("reads a task with no trackers field as tracked by nobody", () => {
    expect(tracks(task(), ME)).toBe(false);
    expect(entries([task({ assignee: null })])).toEqual([]);
  });
});

describe("what counts as over", () => {
  // Closing and the Done column are independent, and either one makes the
  // task history to a reader asking what an agent is doing now.
  it("is closed anywhere, or open in Done", () => {
    expect(isFinished(task({ state: "closed", status: "backlog" }))).toBe(true);
    expect(isFinished(task({ state: "open", status: "done" }))).toBe(true);
    expect(isFinished(task({ state: "open", status: "in_review" }))).toBe(false);
  });
});

describe("where a task stands for this agent", () => {
  it("puts what it is doing above what it is merely holding", () => {
    const tasks = [
      mine({ number: 1, status: "backlog" }),
      mine({ number: 2, status: "in_progress" }),
      mine({ number: 4, status: "ready" }),
    ];
    expect(entries(tasks)).toEqual(["in_progress:2", "assigned:4", "assigned:1"]);
  });

  // #34: on an AGENT's list In review is finished work. The agent has said it
  // is ready to be looked at and has nothing more to do with it — which is not
  // the same question as whether the task is open.
  it("calls In review, Done and Closed the three it is finished with", () => {
    const tasks = [
      mine({ number: 1, state: "closed", status: "in_progress" }),
      mine({ number: 2, status: "done" }),
      mine({ number: 3, status: "in_review" }),
      mine({ number: 4, status: "in_progress" }),
    ];
    expect(entries(tasks)).toEqual(["in_progress:4", "in_review:3", "done:2", "closed:1"]);
  });

  it("tracks what it follows and does not hold", () => {
    const tasks = [task({ number: 9, assignee: { kind: "agent", agent_id: THEM }, trackers: [ME] })];
    expect(entries(tasks)).toEqual(["tracked:9"]);
  });

  // The assignee is tracked automatically (#13), so almost every assigned task
  // is also a tracked one. Listing it twice would double every row.
  it("never lists a task twice when it is both assigned and tracked", () => {
    expect(entries([mine({ number: 5, status: "in_progress", trackers: [ME] })])).toEqual(["in_progress:5"]);
  });

  it("ignores a task that is neither assigned to nor tracked by this agent", () => {
    expect(entries([task({ number: 1, assignee: { kind: "agent", agent_id: THEM } })])).toEqual([]);
  });

  it("answers nothing for no agent, and nothing for no tasks", () => {
    expect(agentTaskEntries([mine({ number: 1 })], null)).toEqual([]);
    expect(agentTaskEntries([], ME)).toEqual([]);
    expect(agentTaskEntries(null, ME)).toEqual([]);
    expect(groupOf(mine({ number: 1 }), "")).toBeNull();
  });

  it("carries what a row needs and nothing raw beside it", () => {
    const [row] = agentTaskEntries([mine({ number: 7, id: "i7", title: "Kanban", status: "in_progress" })], ME);
    // `state` is the surface's, not the task's own open/closed — two meanings
    // on one field is a bug waiting for somebody to read the wrong one.
    expect(row).toEqual({ id: "i7", state: "in_progress", number: 7, title: "Kanban", status: "in_progress", updated_at: "2026-08-21T10:00:00Z" });
  });
});

describe("the order within one standing", () => {
  // An activity surface answers "what has moved", not "what was filed" — the
  // Tasks tab's newest-first order is a catalogue's, and is not this.
  it("puts what moved most recently first", () => {
    const tasks = [
      mine({ number: 1, status: "ready", updated_at: "2026-08-21T10:00:00Z" }),
      mine({ number: 2, status: "ready", updated_at: "2026-08-23T10:00:00Z" }),
      mine({ number: 3, status: "ready", updated_at: "2026-08-22T10:00:00Z" }),
    ];
    expect(numbersIn(tasks, "assigned")).toEqual([2, 3, 1]);
  });

  it("breaks a tie by number, so every repaint agrees", () => {
    const same = "2026-08-21T10:00:00Z";
    const tasks = [
      mine({ number: 7, status: "ready", updated_at: same }),
      mine({ number: 9, status: "ready", updated_at: same }),
      mine({ number: 8, status: "ready", updated_at: same }),
    ];
    expect(numbersIn(tasks, "assigned")).toEqual([9, 8, 7]);
  });

  // 0 is "unknown", not "now": a task with no readable stamp sorts oldest
  // rather than jumping to the top.
  it("sorts an unreadable stamp oldest rather than newest", () => {
    const tasks = [
      mine({ number: 1, status: "ready", updated_at: "not a date" }),
      mine({ number: 2, status: "ready", updated_at: "2026-08-21T10:00:00Z" }),
    ];
    expect(numbersIn(tasks, "assigned")).toEqual([2, 1]);
  });
});

describe("the badge's count", () => {
  const tasks = [
    mine({ number: 1, status: "in_progress" }),
    mine({ number: 2, status: "ready" }),
    mine({ number: 3, status: "done" }),
    task({ number: 4, assignee: { kind: "agent", agent_id: THEM }, trackers: [ME] }),
    task({ number: 5, assignee: { kind: "agent", agent_id: THEM } }),
  ];

  // What a badge asks: is this agent carrying anything right now. Deliberately
  // NOT the surface's reading of finished — In review still counts as carried
  // here, because the badge is asked by somebody looking at the workspace, not
  // by somebody reading the agent's own conversation.
  it("counts only what it is holding and not finished", () => {
    expect(agentOpenTaskCount(tasks, ME)).toBe(2);
  });

  it("counts nothing for an agent with nothing", () => {
    expect(agentOpenTaskCount(tasks, "agent-nobody")).toBe(0);
    expect(agentOpenTaskCount([], ME)).toBe(0);
  });
});
