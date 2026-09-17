import { describe, it, expect } from "vitest";
import { projectPageModel } from "../src/core/projectPageModel.js";

// One device's feed, stamped the way core/feedMerge.js stamps it: every row
// knows the machine that answered, and a project is named by the pair.
const workspace = (id, extra = {}) => ({
  id,
  workspace_id: id,
  project_id: "proj-1",
  deviceId: "dev-1",
  projectKey: "dev-1/proj-1",
  workspaceKey: `dev-1/${id}`,
  status: "ready",
  name: id,
  updated_at: "2026-01-01T00:00:00Z",
  ...extra,
});

const feed = {
  projects: [{ id: "proj-1", project_id: "proj-1", name: "Build", deviceId: "dev-1", projectKey: "dev-1/proj-1" }],
  workspaces: [
    workspace("ws-1", { directories: [{ source_id: "repo", branch: "build/login", is_git: true }] }),
    workspace("ws-2", { status: "failed", directories: [{ source_id: "repo", error: "clone refused" }] }),
  ],
  items: [],
};

const route = { name: "project", deviceId: "dev-1", projectId: "proj-1" };

describe("projectPageModel", () => {
  it("names the project the route stands in", () => {
    const page = projectPageModel(feed, route);
    expect(page.name).toBe("Build");
    expect(page.projectId).toBe("proj-1");
    expect(page.deviceId).toBe("dev-1");
    expect(page.projectKey).toBe("dev-1/proj-1");
    expect(page.empty).toBe(false);
  });

  // The rail already groups every device's workspaces into project blocks; the
  // page is one of those blocks opened on its own, so a row here and the same
  // row on the rail carry the same route.
  it("lists that project's workspaces, each opening its own surface", () => {
    const rows = projectPageModel(feed, route).rows;
    expect(rows.map((row) => row.workspaceId)).toEqual(["ws-1", "ws-2"]);
    expect(rows[0].route).toEqual({
      name: "workspace", deviceId: "dev-1", projectId: "proj-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes",
    });
    expect(rows[0].key).toBe("workspace:dev-1/ws-1");
  });

  it("says what each workspace is standing on and how it is doing", () => {
    const [ready, failed] = projectPageModel(feed, route).rows;
    expect(ready.branch).toBe("build/login");
    expect(ready.status).toBe("ready");
    expect(ready.statusText).toBe("ready");
    expect(failed.branch).toBe("");
    expect(failed.status).toBe("failed");
    expect(failed.statusText).toContain("clone refused");
  });

  it("carries the unread each workspace is owed", () => {
    const unread = {
      ...feed,
      items: [{
        kind: "branch", project_id: "proj-1", deviceId: "dev-1", projectKey: "dev-1/proj-1",
        entity_id: "run-1", unread: true, unread_count: 3, unread_reason: "done",
      }],
      workspaces: [workspace("ws-1", { entity_id: "run-1" })],
    };
    const [row] = projectPageModel(unread, route).rows;
    expect(row.unreadCount).toBe(3);
    expect(row.state).toBe("unread");
    expect(projectPageModel(unread, route).unreadCount).toBe(3);
  });

  // A project nobody has cut a workspace in yet is the ordinary first state of
  // a project, not a missing one: it is named, and it is empty.
  it("is empty, and still named, for a project with no workspaces", () => {
    const page = projectPageModel({ ...feed, workspaces: [] }, route);
    expect(page.rows).toEqual([]);
    expect(page.empty).toBe(true);
    expect(page.name).toBe("Build");
  });

  // Both machines mint a `proj-1`. A page standing in one of them must never
  // list the other's workspaces, and a route that names no machine names no
  // project at all.
  it("keeps to the machine the route names", () => {
    const elsewhere = {
      ...feed,
      workspaces: [
        ...feed.workspaces,
        { ...workspace("ws-9"), deviceId: "dev-2", projectKey: "dev-2/proj-1", workspaceKey: "dev-2/ws-9" },
      ],
    };
    expect(projectPageModel(elsewhere, route).rows.map((row) => row.workspaceId)).toEqual(["ws-1", "ws-2"]);
    const nowhere = projectPageModel(feed, { name: "project", projectId: "proj-1" });
    expect(nowhere.rows).toEqual([]);
    expect(nowhere.projectKey).toBeNull();
  });

  it("falls back to the id for a project no device has listed", () => {
    const page = projectPageModel({ ...feed, projects: [], workspaces: [] }, route);
    expect(page.name).toBe("proj-1");
    expect(page.empty).toBe(true);
  });

  it("stands up on a feed carrying nothing at all", () => {
    expect(projectPageModel(undefined, route).rows).toEqual([]);
    expect(projectPageModel({}, route).empty).toBe(true);
  });
});
