// The view-area toolbar's pure model: the identity it prints, the two menus its
// two selectors open, and the status its right side reports.

import { describe, it, expect } from "vitest";
import {
  branchNamePreview,
  projectMenuModel,
  toolbarIdentity,
  workMenuModel,
  workspaceDirectoryModel,
  workspaceMenuModel,
} from "../src/core/toolbarModel.js";
import { deviceTagHtml } from "../src/core/inboxProjects.js";

const NOW = Date.parse("2026-08-13T12:00:00Z");
const ago = (seconds) => new Date(NOW - seconds * 1000).toISOString();

const branchRow = (over = {}) => ({
  kind: "branch",
  deviceId: "dev-1",
  projectKey: "dev-1/p1",
  project_id: "p1",
  project: "relaydb",
  branch: "build/login",
  title: "Fix the login flow",
  state: "building",
  unread: false,
  working: true,
  working_time: { since: ago(750), seconds: 750 },
  stat: { files_changed: 3, insertions: 42, deletions: 7 },
  resume_at: ago(60),
  issue_id: null,
  ...over,
});

const issueRow = (over = {}) => ({
  kind: "issue",
  deviceId: "dev-1",
  projectKey: "dev-1/p1",
  project_id: "p1",
  project: "relaydb",
  branch: null,
  title: "Add a health endpoint",
  state: "created",
  unread: false,
  working: false,
  working_time: null,
  stat: null,
  resume_at: ago(30),
  issue_id: "plan-1",
  ...over,
});

const on = (deviceId, project) => ({ ...project, deviceId, projectKey: `${deviceId}/${project.id}` });

const projects = [on("dev-1", { id: "p1", name: "relaydb" }), on("dev-1", { id: "p2", name: "mascot" })];

const devices = [
  { id: "dev-1", name: "workshop" },
  { id: "dev-2", name: "laptop" },
];

describe("what the toolbar says you are standing in", () => {
  const feed = { items: [branchRow(), issueRow()], projects };

  it("names the project and the branch from the route, with the feed's words", () => {
    const identity = toolbarIdentity({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" }, feed);
    expect(identity).toMatchObject({ projectId: "p1", project: "relaydb", kind: "branch", label: "build/login" });
    expect(identity.row).toBe(feed.items[0]);
  });

  it("names an issue by its title", () => {
    const identity = toolbarIdentity({ name: "issue", deviceId: "dev-1", projectId: "p1", id: "plan-1" }, feed);
    expect(identity).toMatchObject({ kind: "issue", label: "Add a health endpoint", project: "relaydb" });
  });

  it("still names a branch the feed has not caught up with", () => {
    const identity = toolbarIdentity({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/fresh" }, feed);
    expect(identity).toMatchObject({ label: "build/fresh", project: "relaydb", row: null });
  });

  // Two machines each hold a `p1` with a `build/login` in it: the row the bar
  // names is the one on the machine the route stands on.
  it("toolbarIdentity matches the row on the route's device", () => {
    const theirs = branchRow({ deviceId: "dev-2", projectKey: "dev-2/p1", title: "Their login flow" });
    const both = { items: [...feed.items, theirs], projects: [...projects, on("dev-2", { id: "p1", name: "relaydb" })] };
    const identity = toolbarIdentity({ name: "branch", deviceId: "dev-2", projectId: "p1", branch: "build/login" }, both);
    expect(identity.row).toBe(theirs);
    expect(toolbarIdentity({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" }, both).row).toBe(
      feed.items[0],
    );
  });

  it("names nothing on a route that is not a work item", () => {
    expect(toolbarIdentity({ name: "inbox" }, feed)).toMatchObject({ kind: null, label: "" });
  });

  it("names a plain folder's prospective branch without inventing a board row", () => {
    const folder = on("dev-1", { id: "folder-1", name: "notes", is_git: false, base_branch: "main" });
    const identity = toolbarIdentity(
      { name: "branch", deviceId: "dev-1", projectId: "folder-1", branch: "main", tab: "files" },
      { items: [], projects: [folder] },
    );
    expect(identity).toMatchObject({ projectId: "folder-1", project: "notes", kind: "branch", label: "main", row: null });
  });
});

describe("the project selector's menu", () => {
  const items = [branchRow(), issueRow(), branchRow({ project_id: "p2", projectKey: "dev-1/p2", branch: "build/spike", resume_at: ago(10) })];

  // The rail lists every machine's projects; so does this menu, and the scoped
  // one is named by the pair (device, project) — nothing else names one.
  it("the project menu lists every device's projects and marks the scoped one by projectKey", () => {
    const merged = [...projects, on("dev-2", { id: "p1", name: "relaydb" })];
    const menu = projectMenuModel({ projects: merged, devices, projectKey: "dev-2/p1" });
    expect(menu.map((project) => [project.key, project.current])).toEqual([
      ["dev-1/p1", false],
      ["dev-1/p2", false],
      ["dev-2/p1", true],
    ]);
  });

  // A name the account uses once says which project it is; one two machines
  // both use does not, and the device is said after it — the rail's own rule.
  it("a name clash shows the device name in .dim, a unique name shows none", () => {
    const merged = [...projects, on("dev-2", { id: "p1", name: "relaydb" }), on("dev-2", { id: "p9", name: "notes" })];
    const menu = projectMenuModel({ projects: merged, devices, projectKey: "dev-1/p1" });
    const tagOf = (key) => deviceTagHtml(menu.find((project) => project.key === key));
    expect(tagOf("dev-1/p1")).toBe(' <span class="dim">workshop</span>');
    expect(tagOf("dev-2/p1")).toBe(' <span class="dim">laptop</span>');
    expect(tagOf("dev-2/p9")).toBe("");
    expect(tagOf("dev-1/p2")).toBe("");
  });

  it("counts a project's unread by the projectKey, not the bare id both machines mint", () => {
    const merged = [...projects, on("dev-2", { id: "p1", name: "relaydb" })];
    const rows = [branchRow({ unread: true, unread_count: 2 }), branchRow({ deviceId: "dev-2", projectKey: "dev-2/p1", unread: true, unread_count: 5 })];
    expect(projectMenuModel({ projects: merged, items: rows, devices, projectKey: "dev-1/p1" }).map((project) => project.unreadCount)).toEqual([
      2, 0, 5,
    ]);
  });

  it("carries the projects, and marks the one the toolbar is scoped to", () => {
    expect(projectMenuModel({ projects, projectKey: "dev-1/p1" }).map((project) => [project.name, project.current])).toEqual([
      ["relaydb", true],
      ["mascot", false],
    ]);
  });

  it("carries no work — that is the other menu's list", () => {
    const menu = projectMenuModel({ items, projects, projectKey: "dev-1/p1" });
    expect(menu.every((project) => projects.some((known) => known.id === project.id))).toBe(true);
    expect(menu.map((project) => project.name)).not.toContain("build/login");
  });

  it("filters by project name", () => {
    expect(projectMenuModel({ projects, projectKey: "dev-1/p1", query: "masc" }).map((project) => project.name)).toEqual(["mascot"]);
    // A branch's letters name no project, so the project menu comes back empty.
    expect(projectMenuModel({ projects, projectKey: "dev-1/p1", query: "login" })).toEqual([]);
  });

  it("falls back to the id for a project with no name", () => {
    expect(projectMenuModel({ projects: [on("dev-1", { id: "p9" })], projectKey: "dev-1/p9" })).toEqual([
      { key: "dev-1/p9", id: "p9", deviceId: "dev-1", name: "p9", deviceName: null, clash: false, current: true, unreadCount: 0 },
    ]);
  });
});

describe("the unread each menu counts", () => {
  const captureRow = (over = {}) => ({
    kind: "capture",
    deviceId: "dev-1",
    projectKey: "dev-1/p1",
    project_id: "p1",
    project: "relaydb",
    capture_id: "c1",
    title: "Said on the phone",
    state: "unrouted",
    unread: true,
    unread_count: 1,
    resume_at: ago(5),
    ...over,
  });

  it("counts a project as the sum of its own work's unread", () => {
    const items = [
      branchRow({ unread: true, unread_count: 2 }),
      issueRow({ unread: true, unread_count: 3 }),
      branchRow({ project_id: "p2", projectKey: "dev-1/p2", branch: "build/spike", unread: true, unread_count: 4 }),
    ];
    expect(projectMenuModel({ projects, items, projectKey: "dev-1/p1" }).map((project) => [project.name, project.unreadCount])).toEqual([
      ["relaydb", 5],
      ["mascot", 4],
    ]);
  });

  it("counts nothing for a project whose work has all been read", () => {
    expect(projectMenuModel({ projects, items: [branchRow(), issueRow()], projectKey: "dev-1/p1" }).map((project) => project.unreadCount)).toEqual([
      0, 0,
    ]);
    expect(projectMenuModel({ projects, projectKey: "dev-1/p1" }).map((project) => project.unreadCount)).toEqual([0, 0]);
  });

  it("counts an unread row the bridge sent no count for as one", () => {
    const items = [branchRow({ unread: true, unread_count: 0 }), issueRow({ unread: true })];
    expect(projectMenuModel({ projects, items, projectKey: "dev-1/p1" })[0].unreadCount).toBe(2);
  });

  it("counts a capture waiting on its project, which is where it will land", () => {
    expect(projectMenuModel({ projects, items: [captureRow()], projectKey: "dev-1/p1" })[0].unreadCount).toBe(1);
    // A capture the router has not placed yet belongs to no project, so it is
    // counted against none of them.
    expect(projectMenuModel({ projects, items: [captureRow({ project_id: "", projectKey: null })], projectKey: "dev-1/p1" })[0].unreadCount).toBe(0);
  });

  it("carries each branch's and issue's own count on the work menu", () => {
    const items = [branchRow({ unread: true, unread_count: 2 }), issueRow({ unread: true, unread_count: 3 }), branchRow({ branch: "build/quiet" })];
    expect(workMenuModel({ items, projectKey: "dev-1/p1" }).map((entry) => [entry.label, entry.unreadCount])).toEqual([
      ["Add a health endpoint", 3],
      ["build/login", 2],
      ["build/quiet", 0],
    ]);
  });
});

describe("the branch-and-issue selector's menu", () => {
  const items = [branchRow(), issueRow(), branchRow({ project_id: "p2", projectKey: "dev-1/p2", branch: "build/spike", resume_at: ago(10) })];

  it("carries the work inside the scoped project, and no other project's", () => {
    const work = workMenuModel({ items, projectKey: "dev-1/p1" });
    expect(work.map((entry) => entry.label)).toEqual(["Add a health endpoint", "build/login"]);
    expect(work[1].route).toEqual({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login", tab: "changes" });
    expect(work[0].route).toEqual({ name: "issue", deviceId: "dev-1", projectId: "p1", id: "plan-1" });
  });

  it("follows the scope to another project", () => {
    expect(workMenuModel({ items, projectKey: "dev-1/p2" }).map((entry) => entry.label)).toEqual(["build/spike"]);
  });

  it("filters by what the work is called", () => {
    expect(workMenuModel({ items, projectKey: "dev-1/p1", query: "login" }).map((entry) => entry.label)).toEqual(["build/login"]);
    // An issue is findable by its title, a branch by its letters.
    expect(workMenuModel({ items, projectKey: "dev-1/p1", query: "health" }).map((entry) => entry.kind)).toEqual(["issue"]);
    expect(workMenuModel({ items, projectKey: "dev-1/p1", query: "blgn" }).map((entry) => entry.label)).toEqual(["build/login"]);
    // A project's name is not a work item's, so it filters the work away.
    expect(workMenuModel({ items, projectKey: "dev-1/p1", query: "mascot" })).toEqual([]);
  });

  it("leaves out what no URL can name", () => {
    const detached = branchRow({ branch: null, worktree_id: "wt-9" });
    expect(workMenuModel({ items: [detached, issueRow()], projectKey: "dev-1/p1" }).map((entry) => entry.kind)).toEqual(["issue"]);
  });

  it("has nothing to offer before a project is scoped", () => {
    expect(workMenuModel({ items })).toEqual([]);
  });

  // The account's work, on every machine: two devices each hold a `p1`, and the
  // menu of one of them lists only its own.
  it("the work menu is the scoped projectKey's work only", () => {
    const theirs = branchRow({ deviceId: "dev-2", projectKey: "dev-2/p1", branch: "build/away" });
    expect(workMenuModel({ items: [...items, theirs], projectKey: "dev-2/p1" }).map((entry) => entry.label)).toEqual([
      "build/away",
    ]);
    expect(workMenuModel({ items: [...items, theirs], projectKey: "dev-1/p1" }).map((entry) => entry.label)).toEqual([
      "Add a health endpoint",
      "build/login",
    ]);
  });

  it("menu routes carry deviceId", () => {
    const theirs = branchRow({ deviceId: "dev-2", projectKey: "dev-2/p1", branch: "build/away" });
    const [away] = workMenuModel({ items: [theirs], projectKey: "dev-2/p1" });
    expect(away.route).toEqual({ name: "branch", deviceId: "dev-2", projectId: "p1", branch: "build/away", tab: "changes" });
    expect(away.key).toBe("branch:dev-2/p1:build/away");
  });
});

describe("the branch a typed name becomes", () => {
  it("mirrors the daemon's slug rules", () => {
    expect(branchNamePreview("Mascot Model Spike!")).toBe("build/mascot-model-spike");
    expect(branchNamePreview("  ")).toBe("");
    expect(branchNamePreview("***")).toBe("");
  });
});

describe("workspace navigation", () => {
  const workspaces = [
    {
      id: "ws-1",
      project_id: "p1",
      name: "payment-work",
      directories: [
        { id: "d1", source_id: "frontend", name: "Frontend", is_git: true },
        { id: "d2", source_id: "assets", name: "Design assets", is_git: false },
      ],
    },
    { workspace_id: "ws-2", project_id: "p1", name: "release" },
    { id: "ws-3", project_id: "p2", name: "mascot work" },
  ];

  it("lists only the scoped project's workspaces and normalizes their ids", () => {
    expect(workspaceMenuModel({ workspaces, projectId: "p1", workspaceId: "ws-2" }).map((row) => [row.id, row.current])).toEqual([
      ["ws-1", false],
      ["ws-2", true],
    ]);
    expect(workspaceMenuModel({ workspaces, projectId: "p1", query: "pay" }).map((row) => row.id)).toEqual(["ws-1"]);
  });

  it("turns workspace directories into persistent tab identities", () => {
    expect(workspaceDirectoryModel(workspaces[0], "assets").map((row) => [row.sourceId, row.label, row.current])).toEqual([
      ["frontend", "Frontend", false],
      ["assets", "Design assets", true],
    ]);
  });

  it("names a workspace and its selected directory from the canonical route", () => {
    const identity = toolbarIdentity(
      { name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" },
      { items: [], projects, workspaces },
    );
    expect(identity).toMatchObject({
      projectId: "p1",
      project: "relaydb",
      kind: "workspace",
      label: "payment-work",
      workspaceId: "ws-1",
    });
    expect(identity.directories.map((row) => [row.sourceId, row.current])).toEqual([
      ["frontend", true],
      ["assets", false],
    ]);
  });
});
