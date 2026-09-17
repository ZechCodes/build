// The feed's two pure halves: the one place the board/project wire is read
// (stamping every row with the device that answered), and the merge that makes
// one inbox out of every device's snapshot.
//
// Nothing here touches a session, a cache or the App — the merge is a function
// of the snapshots and the device order, so it is tested as one.

import { describe, it, expect } from "vitest";
import { deviceView, liveFeedSnapshot, mergeFeeds, projectEntityIds, withoutProject } from "../src/core/feedMerge.js";
import { deviceKey } from "../src/core/deviceKey.js";

const board = (over = {}) => ({
  items: [{ kind: "branch", project_id: "proj-1", branch: "build/login" }],
  plans: [{ id: "plan-1", project_id: "proj-1" }],
  runs: [{ run_id: "run-1", project_id: "proj-1" }],
  external_worktrees: [{ worktree_id: "wt-1", project_id: "proj-2" }],
  pending: [{ entity_id: "wt-9", project_id: "proj-1", state: "creating" }],
  workspace_summaries: [{ workspace_id: "ws-1", work_summary: { pushes: 2, additions: 8, deletions: 1 } }],
  ...over,
});

const projectList = { projects: [{ project_id: "proj-1", name: "relaydb" }] };

const workspaceList = {
  workspaces: [
    { workspace_id: "ws-1", project_id: "proj-1", name: "login" },
    { workspace_id: "ws-2", project_id: "proj-2", name: "search" },
  ],
};

describe("reading one device's wire", () => {
  it("stamps every row of every collection with deviceId, and projectKey where it names a project_id", () => {
    const view = liveFeedSnapshot(board(), projectList, workspaceList, "dev-a");
    for (const field of ["items", "plans", "runs", "externalWorktrees", "pending"]) {
      expect(view[field], field).toHaveLength(1);
      expect(view[field][0].deviceId, field).toBe("dev-a");
      expect(view[field][0].projectKey, field).toBe(`dev-a/${view[field][0].project_id}`);
    }
  });

  it("leaves a row that names no project without a project key", () => {
    const issueOnly = board({ items: [{ kind: "issue", issue_id: "iss-1" }] });
    const view = liveFeedSnapshot(issueOnly, projectList, workspaceList, "dev-a");
    expect(view.items[0].deviceId).toBe("dev-a");
    expect("projectKey" in view.items[0]).toBe(false);
  });

  it("stamps every project with deviceId and projectKey and keeps project_id on the wire shape", () => {
    const view = liveFeedSnapshot(board(), projectList, workspaceList, "dev-a");
    expect(view.projects).toEqual([
      { id: "proj-1", project_id: "proj-1", name: "relaydb", deviceId: "dev-a", projectKey: "dev-a/proj-1" },
    ]);
  });

  it("answers empty collections for a bridge that sends none", () => {
    const view = liveFeedSnapshot({}, {}, {}, "dev-a");
    expect(view).toEqual({
      items: [],
      plans: [],
      runs: [],
      externalWorktrees: [],
      pending: [],
      projects: [],
      workspaces: [],
    });
  });

  it("stamps a workspace with its device, its project and its account-wide name", () => {
    const view = liveFeedSnapshot(board(), projectList, workspaceList, "dev-a");
    expect(view.workspaces.map(({ id, deviceId, projectKey, workspaceKey }) => ({
      id,
      deviceId,
      projectKey,
      workspaceKey,
    }))).toEqual([
      { id: "ws-1", deviceId: "dev-a", projectKey: "dev-a/proj-1", workspaceKey: "dev-a/ws-1" },
      { id: "ws-2", deviceId: "dev-a", projectKey: "dev-a/proj-2", workspaceKey: "dev-a/ws-2" },
    ]);
  });

  it("joins the board's work summary onto the workspace it names, and leaves the rest without one", () => {
    const view = liveFeedSnapshot(board(), projectList, workspaceList, "dev-a");
    expect(view.workspaces[0].work_summary).toEqual({ pushes: 2, additions: 8, deletions: 1 });
    expect("work_summary" in view.workspaces[1]).toBe(false);
  });

  it("answers no workspaces for a bridge that does not serve them", () => {
    expect(liveFeedSnapshot(board(), projectList, null, "dev-a").workspaces).toEqual([]);
  });
});

describe("merging the devices", () => {
  const viewOf = (deviceId, over = {}) =>
    liveFeedSnapshot(
      { items: [{ kind: "branch", project_id: "proj-1", branch: deviceId }], ...over },
      projectList,
      { workspaces: [{ workspace_id: `ws-${deviceId}`, project_id: "proj-1" }] },
      deviceId,
    );

  it("concatenates the eight arrays in device order, ids not in the order last", () => {
    const byDevice = new Map([
      ["dev-c", viewOf("dev-c")],
      ["dev-b", viewOf("dev-b")],
      ["dev-a", viewOf("dev-a")],
    ]);
    const merged = mergeFeeds(byDevice, ["dev-a", "dev-b"]);
    expect(merged.items.map((item) => item.deviceId)).toEqual(["dev-a", "dev-b", "dev-c"]);
    expect(merged.projects.map((project) => project.projectKey)).toEqual([
      "dev-a/proj-1",
      "dev-b/proj-1",
      "dev-c/proj-1",
    ]);
    expect(merged.workspaces.map((workspace) => workspace.workspaceKey)).toEqual([
      "dev-a/ws-dev-a",
      "dev-b/ws-dev-b",
      "dev-c/ws-dev-c",
    ]);
    for (const field of ["plans", "runs", "externalWorktrees", "pending"]) {
      expect(merged[field], field).toEqual([]);
    }
  });

  it("exposes each device's view under devices[id]", () => {
    const a = viewOf("dev-a");
    const merged = mergeFeeds(new Map([["dev-a", a]]), ["dev-a"]);
    expect(merged.devices["dev-a"]).toBe(a);
  });

  it("is cached only when every entry is cached", () => {
    const live = viewOf("dev-a");
    const cached = { ...viewOf("dev-b"), cached: true };
    expect(mergeFeeds(new Map([["dev-a", live], ["dev-b", cached]]), ["dev-a", "dev-b"]).cached).toBe(false);
    expect(mergeFeeds(new Map([["dev-b", cached]]), ["dev-b"]).cached).toBe(true);
  });
});

describe("one device's view of a merge", () => {
  it("reads the named device's view out of the merge", () => {
    const a = liveFeedSnapshot(board(), projectList, workspaceList, "dev-a");
    const b = liveFeedSnapshot({}, {}, {}, "dev-b");
    const merged = mergeFeeds(new Map([["dev-a", a], ["dev-b", b]]), ["dev-a", "dev-b"]);
    expect(deviceView(merged, "dev-a")).toBe(a);
    expect(deviceView(merged, "dev-b")).toBe(b);
  });

  it("takes a snapshot that names no devices for one device's view already", () => {
    const single = { items: [{ kind: "branch" }], projects: [] };
    expect(deviceView(single, "dev-a")).toBe(single);
  });

  it("answers an empty view for a device the merge does not hold", () => {
    const merged = mergeFeeds(new Map(), []);
    expect(deviceView(merged, "dev-a").items).toEqual([]);
    expect(deviceView(null, "dev-a").projects).toEqual([]);
  });
});

// What hiding a project on a gone machine is made of (core/projectHide.js): the
// same pruning runs over the snapshot in memory and the record on disk, so the
// two cannot disagree about what is left.
describe("taking one project's rows out of a view", () => {
  const view = () =>
    liveFeedSnapshot(board(), { projects: [{ project_id: "proj-1" }, { project_id: "proj-2" }] }, workspaceList, "dev-a");

  it("drops that project's rows from every collection and leaves the rest", () => {
    const pruned = withoutProject(view(), deviceKey("dev-a", "proj-1"));
    expect(pruned.items).toEqual([]);
    expect(pruned.plans).toEqual([]);
    expect(pruned.projects.map((row) => row.id)).toEqual(["proj-2"]);
    expect(pruned.workspaces.map((row) => row.id)).toEqual(["ws-2"]);
    // Another project on the same machine keeps everything it had.
    expect(pruned.externalWorktrees.map((row) => row.worktree_id)).toEqual(["wt-1"]);
  });

  // By the account-wide key, never the bare id: both machines mint a `proj-1`,
  // and hiding one must not take the other's rows with it.
  it("is keyed by the account-wide project key rather than the bare id", () => {
    const here = view();
    expect(withoutProject(here, deviceKey("dev-b", "proj-1"))).toEqual(here);
    expect(withoutProject(here, "proj-1")).toEqual(here);
  });

  it("leaves a view alone when there is no project to name", () => {
    const here = view();
    expect(withoutProject(here, null)).toBe(here);
    expect(withoutProject(null, "dev-a/proj-1")).toBeNull();
  });

  // The cache is keyed by (device, entity) and a project is not an entity of
  // its own, so hiding one has to name every entity its rows are cached under.
  it("names every id a project's rows are cached under", () => {
    const ids = projectEntityIds(view(), deviceKey("dev-a", "proj-1"));
    expect(new Set(ids)).toEqual(new Set(["plan-1", "run-1", "wt-9", "ws-1", "proj-1"]));
    expect(projectEntityIds(view(), deviceKey("dev-a", "nothing"))).toEqual([]);
    expect(projectEntityIds(null, "dev-a/proj-1")).toEqual([]);
  });
});
