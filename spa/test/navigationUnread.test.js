import { describe, expect, it } from "vitest";
import { liveFeedSnapshot } from "../src/core/feedMerge.js";
import { navigationUnreadCounts, navigationUnreadEntries } from "../src/core/navigationUnread.js";
import { taskUnreadTally } from "../src/core/taskUnread.js";
import { task } from "./trackerWireFixture.js";

const projectKey = "device/project-1";
const workspaceKey = "device/workspace-1";
const agent = (id, unread_count, watched = true) => ({ id, unread_count, watched });

function snapshot({ workspaceAgents = [agent("worker", 2)], projectAgents = [agent("planner", 1)] } = {}) {
  return liveFeedSnapshot({
    items: [{ kind: "branch", project_id: "project-1", run_id: "workspace-run", agents: workspaceAgents }],
    runs: [{ project_id: "project-1", run_id: "project-run", agents: projectAgents }],
  }, { projects: [{ id: "project-1", name: "Build", entity_id: "project-run" }] }, {
    workspaces: [{ id: "workspace-1", name: "Navigation", project_id: "project-1", entity_id: "workspace-run", status: "ready" }],
  }, "device");
}

function withTasks(view, tasks) {
  return { ...view, taskUnread: taskUnreadTally([{ project: view.projects[0], tasks }]) };
}

describe("shared navigation unread attribution", () => {
  it("counts a watched task on its workspace once in the project aggregate", () => {
    const view = withTasks(snapshot(), [task({ watched: true, unread_count: 3,
      assignee: { kind: "agent", agent_id: "worker" } })]);
    const counts = navigationUnreadCounts(view);
    expect(counts.workspaces.get(workspaceKey)).toBe(5);
    expect(counts.projectOwn.get(projectKey)).toBe(1);
    expect(counts.projects.get(projectKey)).toBe(6);
    const rows = navigationUnreadEntries(view);
    expect(rows.map((row) => row.unreadCount)).toEqual([5, 1]);
  });

  it("attributes unheld watched tasks to the project destination", () => {
    const view = withTasks(snapshot(), [task({ watched: true, unread_count: 4 })]);
    const counts = navigationUnreadCounts(view);
    expect(counts.projectOwn.get(projectKey)).toBe(5);
    expect(counts.workspaces.get(workspaceKey)).toBe(2);
    expect(counts.projects.get(projectKey)).toBe(7);
  });

  it("reads the pinned project agent from runs even when absent from items", () => {
    const counts = navigationUnreadCounts(snapshot({ projectAgents: [agent("planner", 7)] }));
    expect(counts.projectOwn.get(projectKey)).toBe(7);
    expect(counts.projects.get(projectKey)).toBe(9);
  });

  it("shows zero for unwatched destinations without adding their stored counts", () => {
    const view = snapshot({ workspaceAgents: [agent("worker", 22, false)], projectAgents: [agent("planner", 11, false)] });
    const counts = navigationUnreadCounts(view);
    expect(counts.workspaces.get(workspaceKey)).toBe(0);
    expect(counts.projectOwn.get(projectKey)).toBe(0);
    expect(counts.projects.get(projectKey)).toBe(0);
    expect(navigationUnreadEntries(view).some((row) => row.kind === "workspace")).toBe(false);
  });

  it("keeps identical wire IDs on separate devices", () => {
    const first = snapshot();
    const second = liveFeedSnapshot({ items: [{ kind: "branch", project_id: "project-1", run_id: "workspace-run",
      agents: [agent("worker", 9)] }] }, { projects: [{ id: "project-1", name: "Build" }] }, {
      workspaces: [{ id: "workspace-1", project_id: "project-1", entity_id: "workspace-run", status: "ready" }],
    }, "other-device");
    const combined = Object.fromEntries(["items", "runs", "projects", "workspaces"].map((key) => [key, [...first[key], ...second[key]]]));
    const counts = navigationUnreadCounts(combined);
    expect(counts.workspaces.get(workspaceKey)).toBe(2);
    expect(counts.workspaces.get("other-device/workspace-1")).toBe(9);
    expect(counts.projects.get(projectKey)).toBe(3);
    expect(counts.projects.get("other-device/project-1")).toBe(9);
  });
});
