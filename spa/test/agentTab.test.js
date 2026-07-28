// @vitest-environment jsdom
// The Agent tab is a FIXTURE, not a thing you open.
//
// A worktree has one agent, Build owns it, and it is always reachable — so
// every worktree-backed surface carries the same tab in the same place,
// whether or not a session is live in that directory right now. These pin the
// tab rows themselves; surfaceTabs.test.js pins what mounting one does (and
// does not) start.

import { describe, it, expect } from "vitest";
import { AGENT_TAB, NEW_TAB_KINDS } from "../src/core/surfaceTabs.js";
import { projectSurfaceTabs } from "../src/views/mainWorktree.js";
import { worktreeSurfaceTabs } from "../src/views/worktree.js";
import { taskSurfaceTabs } from "../src/views/task.js";

const ids = (tabs) => tabs.map((tab) => tab.id);
const TERMINAL = { id: "term-1", label: "Terminal 1", closable: true };

describe("the Agent tab on every worktree surface", () => {
  it("sits after Files and before the user's terminals, on all three surfaces", () => {
    expect(ids(taskSurfaceTabs({ terminalTabs: [TERMINAL] }))).toEqual(["changes", "files", "agent", "term-1"]);
    expect(ids(worktreeSurfaceTabs([TERMINAL]))).toEqual(["changes", "files", "agent", "term-1"]);
    expect(ids(projectSurfaceTabs([TERMINAL]))).toEqual(["inbox", "issues", "changes", "files", "agent", "term-1"]);
  });

  it("is there with no terminals open and with no run adopted", () => {
    expect(ids(worktreeSurfaceTabs())).toContain("agent");
    expect(ids(projectSurfaceTabs())).toContain("agent");
    expect(ids(taskSurfaceTabs())).toContain("agent");
  });

  // Closing it is not an option offered: the human does not own its lifetime,
  // the worktree does.
  it("is never closable, and is the same tab object everywhere", () => {
    for (const tabs of [taskSurfaceTabs(), worktreeSurfaceTabs(), projectSurfaceTabs()]) {
      const agent = tabs.find((tab) => tab.id === "agent");
      expect(agent).toBe(AGENT_TAB);
      expect(agent.closable).toBeUndefined();
    }
  });

  // A multi-stage run adds Stages ahead of everything; the fixture does not move.
  it("keeps its place when a run grows a Stages tab", () => {
    expect(ids(taskSurfaceTabs({ multiStage: true }))).toEqual(["stages", "changes", "files", "agent"]);
  });

  // The `+` mints shells only. An agent tab it could open would be a SECOND
  // agent in the same directory — unmanaged, with no `done` tool and no owner.
  it("is not something the + can open a second of", () => {
    expect(NEW_TAB_KINDS.map((kind) => kind.id)).toEqual(["shell"]);
  });
});
