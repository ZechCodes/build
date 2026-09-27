// How many open tasks a workspace's agents are holding: the number on the
// icon beside the cog.
//
// The grouping this file used to test went with the #16 overlay (#29): the
// tasks are a tab of the workspace now, drawn by the tracker's own list and
// board, and what those show is tested in workspaceTasksTab(Dom).

import { describe, expect, it } from "vitest";
import { workspaceAgentIds, workspaceOpenTaskCount } from "../src/core/trackerWorkspaceTasks.js";
import { task } from "./trackerWireFixture.js";

const ONE = "agent-01M2ONE";
const TWO = "agent-01M2TWO";
const ELSEWHERE = "agent-01M2ELSE";

const agents = [{ id: ONE, ordinal: 1 }, { id: TWO, ordinal: 2 }];
const held = (agentId, over = {}) => task({ assignee: { kind: "agent", agent_id: agentId }, ...over });

describe("the workspace's agents", () => {
  it("are the ids its row carries", () => {
    expect(workspaceAgentIds(agents)).toEqual([ONE, TWO]);
  });

  // A workspace nobody has spoken in has no row and no agents, which is not an
  // error — it holds no tasks for the same reason.
  it("are none for a workspace with no conversation yet", () => {
    expect(workspaceAgentIds(null)).toEqual([]);
    expect(workspaceAgentIds([{ ordinal: 1 }])).toEqual([]);
  });
});

describe("the badge's count", () => {
  const tasks = [
    held(ONE, { number: 1, status: "in_progress" }),
    held(ONE, { number: 2, status: "ready" }),
    held(TWO, { number: 3, status: "in_review" }),
    held(TWO, { number: 4, status: "done" }),
    held(ONE, { number: 5, state: "closed" }),
    held(ELSEWHERE, { number: 6, status: "in_progress" }),
  ];

  // A badge is a call to look, and finished work is not one.
  it("counts what is open across the workspace's agents, and nothing finished", () => {
    expect(workspaceOpenTaskCount(tasks, agents)).toBe(3);
  });

  it("counts nothing when the agents hold nothing open", () => {
    expect(workspaceOpenTaskCount([held(ONE, { number: 1, status: "done" })], agents)).toBe(0);
    expect(workspaceOpenTaskCount([], agents)).toBe(0);
    expect(workspaceOpenTaskCount(tasks, [])).toBe(0);
  });
});
