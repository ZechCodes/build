import { describe, it, expect } from "vitest";
import {
  ALL_WORKSPACES,
  RECLAIMABLE_WORKSPACES,
  projectPageModel,
  workspaceListing,
} from "../src/core/projectPageModel.js";

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

// #167: the Workspaces tab narrows to what can be reclaimed, biggest first,
// and every row says what it weighs, all from the cached workspace.list rows.
describe("the workspaces listing", () => {
  const verdict = (reclaimable, size) => ({
    idle: true, reclaimable, holds: reclaimable ? [] : ["dirty"], dirty_files: 1, size_bytes: size, pruned_bytes: 0,
  });
  const sized = {
    ...feed,
    workspaces: [
      workspace("small", { lifecycle: verdict(true, 2_000_000_000) }),
      workspace("held", { lifecycle: verdict(false, 40_000_000_000) }),
      workspace("large", { lifecycle: verdict(true, 17_200_000_000) }),
      workspace("unmeasured"),
    ],
  };
  const page = () => projectPageModel(sized, route);

  it("says each row's size, and nothing for one not yet measured", () => {
    const bySize = Object.fromEntries(page().rows.map((row) => [row.workspaceId, row.sizeText]));
    expect(bySize).toEqual({ small: "2.0 GB", held: "40.0 GB", large: "17.2 GB", unmeasured: "" });
  });

  it("lists every workspace in the rail's order unless narrowed", () => {
    const listed = workspaceListing(page(), ALL_WORKSPACES);
    expect(listed.rows.map((row) => row.workspaceId)).toEqual(["small", "held", "large", "unmeasured"]);
    expect(listed.reclaimableCount).toBe(2);
  });

  it("narrows to the reclaimable ones, largest first", () => {
    const listed = workspaceListing(page(), RECLAIMABLE_WORKSPACES);
    expect(listed.rows.map((row) => row.workspaceId)).toEqual(["large", "small"]);
    expect(listed.empty).toBe(false);
  });

  it("is empty when nothing can be reclaimed", () => {
    const listed = workspaceListing(projectPageModel(feed, route), RECLAIMABLE_WORKSPACES);
    expect(listed.rows).toEqual([]);
    expect(listed.empty).toBe(true);
    expect(listed.reclaimableCount).toBe(0);
  });

  it("reads an unknown filter as every workspace", () => {
    expect(workspaceListing(page(), "bogus").rows).toHaveLength(4);
  });
});
