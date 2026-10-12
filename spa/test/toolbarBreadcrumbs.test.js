import { describe, expect, it } from "vitest";
import { toolbarBreadcrumbs } from "../src/core/toolbarBreadcrumbs.js";
import { hashFromRoute } from "../src/core/router.js";

const workspace = { name: "workspace", deviceId: "device", projectId: "project", workspaceId: "child", tab: "tasks", taskId: "task", view: "board", agent: "agent" };
describe("URL-owned toolbar ancestry", () => {
  it("keeps the workspace task collection and exact current link", () => {
    const hash = `${hashFromRoute(workspace)}&fixture=preserve`;
    const crumbs = toolbarBreadcrumbs({ route: workspace, currentHash: hash, project: "Build", label: "Child", task: { number: 542 } });
    expect(crumbs.tasks.route).toEqual({ ...workspace, taskId: undefined });
    expect(crumbs.leaf).toMatchObject({ label: "#542", hash, current: true });
    expect(crumbs.project.current).toBe(false);
  });
  it("uses the project URL host regardless of task workspace metadata", () => {
    const route = { name: "trackerTask", deviceId: "other", projectId: "project", taskId: "task" };
    const crumbs = toolbarBreadcrumbs({ route, project: "Build", task: { number: 542, workspace_id: "child" } });
    expect(crumbs.tasks.route).toEqual({ name: "project", deviceId: "other", projectId: "project", tab: "tasks" });
    expect(crumbs.workspace).toBeNull();
  });
  it("uses task ID until metadata arrives and keeps a slash branch as one current leaf", () => {
    expect(toolbarBreadcrumbs({ route: workspace, project: "Build", label: "Child" }).leaf.label).toBe("task");
    const route = { name: "branch", deviceId: "device", projectId: "project", branch: "build/foo/bar", tab: "files", file: "README.md" };
    const crumbs = toolbarBreadcrumbs({ route, project: "Build", label: route.branch });
    expect(crumbs.leaf).toMatchObject({ label: "build/foo/bar", route, hash: hashFromRoute(route), current: true });
    expect(crumbs.tasks).toBeNull();
  });
});
