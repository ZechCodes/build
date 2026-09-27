// What a task's links open. Nothing here invents a destination: every row is
// a route the app already writes, or it is not a link at all.

import { describe, expect, it } from "vitest";
import { taskLinkRows } from "../src/core/trackerLinks.js";

const PROJECT_KEY = "dev-1|proj-1";
const HERE = { projectId: "proj-1", deviceId: "dev-1", projectKey: PROJECT_KEY };

const feed = {
  projects: [{ projectKey: PROJECT_KEY, entity_id: "run-9" }],
  workspaces: [
    { id: "ws-1", workspace_id: "ws-1", name: "wire-facade", projectKey: PROJECT_KEY, entity_id: "run-1" },
    { id: "ws-2", workspace_id: "ws-2", name: "tasks-board", projectKey: PROJECT_KEY, entity_id: "run-2" },
  ],
};

const linksOf = (links, given = feed) =>
  taskLinkRows({ links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null, ...links } }, HERE, given);

describe("what a task links", () => {
  it("opens a workspace by name, on the machine the project is on", () => {
    expect(linksOf({ workspace_ids: ["ws-1"] })).toEqual([
      {
        kind: "workspace",
        workspaceId: "ws-1",
        label: "wire-facade",
        route: { name: "workspace", projectId: "proj-1", deviceId: "dev-1", workspaceId: "ws-1", tab: "changes" },
      },
    ]);
  });

  it("opens a branch through the branch surface", () => {
    expect(linksOf({ branches: ["build/tasks-spa"] })[0].route).toEqual({
      name: "branch", projectId: "proj-1", deviceId: "dev-1", branch: "build/tasks-spa", tab: "changes",
    });
  });

  // A dispatch records the conversation it delivered into, so a task page can
  // open the conversation that is working it.
  it("opens a conversation on the workspace that owns it", () => {
    const [row] = linksOf({ conversation_ids: ["run-2"] });
    expect([row.label, row.route]).toEqual([
      "tasks-board · conversation",
      { name: "workspace", projectId: "proj-1", deviceId: "dev-1", workspaceId: "ws-2", agent: undefined, tab: "changes" },
    ]);
  });

  // The project's own agent has a conversation too, and it lives on the
  // project page.
  it("opens the project's own conversation on the project page", () => {
    const [row] = linksOf({ conversation_ids: ["run-9"] });
    expect([row.label, row.route.name]).toEqual(["Project agent · conversation", "project"]);
  });

  // No surface is addressed by a bare hash, and a dead link is worse than a
  // fact.
  it("shows a commit rather than linking it", () => {
    expect(linksOf({ commits: ["c8381faa9e1b2c3d4e5f60718293a4b5c6d7e8f9"] })).toEqual([
      { kind: "commit", label: "c8381fa", title: "c8381faa9e1b2c3d4e5f60718293a4b5c6d7e8f9", route: null },
    ]);
  });

  it("opens a parent task on its own page", () => {
    expect(linksOf({ parent_task_id: "task-01K5A" })[0].route).toEqual({
      name: "trackerTask", projectId: "proj-1", deviceId: "dev-1", taskId: "task-01K5A",
    });
  });

  it("keeps the order the record holds, kind by kind", () => {
    const rows = linksOf({
      workspace_ids: ["ws-1"], branches: ["a", "b"], commits: ["abc"], conversation_ids: ["run-1"],
      parent_task_id: "task-9",
    });
    expect(rows.map((row) => row.kind)).toEqual([
      "workspace", "branch", "branch", "conversation", "commit", "parent",
    ]);
  });

  // A deleted workspace keeps its id visible, without an unusable route.
  it("shows deleted workspace and conversation destinations as plain text", () => {
    expect(linksOf({ workspace_ids: ["ws-9"] }, null)[0]).toMatchObject({ label: "ws-9", route: null });
    expect(linksOf({ conversation_ids: ["run-404"] }, feed)[0].route).toBeNull();
  });

  it("has nothing to say about a task that links nothing", () => {
    expect(linksOf({})).toEqual([]);
    expect(taskLinkRows(null, HERE, feed)).toEqual([]);
  });
});
