// Which issues belong on a workspace's Issues tab, and where one opens from
// inside it (#29).
//
// The narrowing is the whole of what makes this tab different from the
// project's own: the same tracker, restricted to the agents standing HERE.

import { describe, expect, it } from "vitest";
import { agentIdsOfWorkspace, heldHere, workspaceIssueRoute, workspaceIssuesPlace } from "../src/core/workspaceIssuesTab.js";
import { issue } from "./trackerWireFixture.js";

const PROJECT_KEY = "dev-1/proj-1";
const HERE = "agent-01M2HERE";
const ALSO = "agent-01M2ALSO";
const AWAY = "agent-01M2AWAY";

const feed = {
  workspaces: [
    { id: "ws-1", workspace_id: "ws-1", name: "issues-spa", projectKey: PROJECT_KEY, entity_id: "run-1" },
    { id: "ws-2", workspace_id: "ws-2", name: "elsewhere", projectKey: PROJECT_KEY, entity_id: "run-2" },
  ],
  items: [
    { kind: "branch", projectKey: PROJECT_KEY, run_id: "run-1", agents: [{ id: HERE, ordinal: 1 }, { id: ALSO, ordinal: 2 }] },
    { kind: "branch", projectKey: PROJECT_KEY, run_id: "run-2", agents: [{ id: AWAY, ordinal: 1 }] },
  ],
};

const held = (agentId, over = {}) => issue({ assignee: { kind: "agent", agent_id: agentId }, ...over });

describe("the agents standing in this workspace", () => {
  it("are the ones its row carries, and nobody else's", () => {
    expect(agentIdsOfWorkspace(feed, PROJECT_KEY, "ws-1")).toEqual([HERE, ALSO]);
    expect(agentIdsOfWorkspace(feed, PROJECT_KEY, "ws-2")).toEqual([AWAY]);
  });

  // A workspace nobody has spoken in has no row and no agents, which is not an
  // error — it holds no issues for the same reason.
  it("are none for a workspace with no conversation, and none for one that is not there", () => {
    expect(agentIdsOfWorkspace(feed, PROJECT_KEY, "ws-9")).toEqual([]);
    expect(agentIdsOfWorkspace(null, PROJECT_KEY, "ws-1")).toEqual([]);
  });
});

describe("which issues belong here", () => {
  const ids = [HERE, ALSO];

  it("are the ones an agent of this workspace is holding", () => {
    expect(heldHere(held(HERE), ids)).toBe(true);
    expect(heldHere(held(ALSO), ids)).toBe(true);
  });

  it("are not the ones an agent of another workspace is holding", () => {
    expect(heldHere(held(AWAY), ids)).toBe(false);
  });

  // The user and the project agent are not standing in any workspace, so what
  // they hold is not this workspace's work.
  it("are not the ones nobody here holds", () => {
    expect(heldHere(issue({ assignee: null }), ids)).toBe(false);
    expect(heldHere(issue({ assignee: { kind: "user" } }), ids)).toBe(false);
    expect(heldHere(issue({ assignee: { kind: "project_agent" } }), ids)).toBe(false);
  });

  // Tracking is context for reading what an agent says and belongs in its
  // conversation; this tab answers "what is the work here".
  it("are not the ones an agent here merely tracks", () => {
    expect(heldHere(issue({ assignee: { kind: "agent", agent_id: AWAY }, trackers: [HERE] }), ids)).toBe(false);
  });

  it("are none at all in a workspace with no agents", () => {
    expect(heldHere(held(HERE), [])).toBe(false);
  });
});

describe("where the tab and its issues live", () => {
  const route = {
    name: "workspace", deviceId: "dev-1", projectId: "proj-1", workspaceId: "ws-1",
    sourceId: "repo", tab: "issues",
  };

  // Still `name: "workspace"`, which is what keeps the shell's rail standing
  // across the switch (core/shell.js keys on the name and the workspace id).
  it("stay workspace routes, so the rail beside them never moves", () => {
    expect(workspaceIssuesPlace(route).name).toBe("workspace");
    expect(workspaceIssueRoute(route, "issue-7").name).toBe("workspace");
    expect(workspaceIssueRoute(route, "issue-7").workspaceId).toBe("ws-1");
  });

  // A directory scopes Changes and Files. It has nothing to say about which
  // issues the agents here are holding, so it is left off entirely.
  it("carry no directory", () => {
    expect(workspaceIssuesPlace(route).sourceId).toBeUndefined();
    expect(workspaceIssueRoute(route, "issue-7").sourceId).toBeUndefined();
  });

  it("name the issue when one is open", () => {
    expect(workspaceIssueRoute(route, "issue-7").issueId).toBe("issue-7");
    expect(workspaceIssuesPlace(route).issueId).toBeUndefined();
  });
});
