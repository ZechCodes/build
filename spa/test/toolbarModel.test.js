// The view-area toolbar's pure model: the identity it prints, the two menus its
// two selectors open, and the status its right side reports.

import { describe, it, expect } from "vitest";
import {
  branchNamePreview,
  humanDuration,
  projectMenuModel,
  statText,
  toolbarIdentity,
  toolbarStatus,
  workMenuModel,
  workingSeconds,
} from "../src/core/toolbarModel.js";

const NOW = Date.parse("2026-08-13T12:00:00Z");
const ago = (seconds) => new Date(NOW - seconds * 1000).toISOString();

const branchRow = (over = {}) => ({
  kind: "branch",
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

const projects = [
  { id: "p1", name: "relaydb" },
  { id: "p2", name: "mascot" },
];

describe("what the toolbar's right side reports", () => {
  it("tickers the working time off the stamp, and falls back to the count", () => {
    expect(workingSeconds({ since: ago(120), seconds: 5 }, NOW)).toBe(120);
    expect(workingSeconds({ since: "not a time", seconds: 5 }, NOW)).toBe(5);
    expect(workingSeconds(null, NOW)).toBeNull();
    expect(humanDuration(30)).toBe("<1m");
    expect(humanDuration(750)).toBe("12m");
    expect(humanDuration(7200)).toBe("2h");
    expect(humanDuration(180000)).toBe("2d");
  });

  it("says the additions and deletions, and nothing when there are none", () => {
    expect(statText({ insertions: 42, deletions: 7 })).toBe("+42 −7");
    expect(statText({ insertions: 0, deletions: 0, files_changed: 0 })).toBe("");
    expect(statText("+4 −1")).toBe("+4 −1");
    expect(statText(null)).toBe("");
  });

  it("reads both halves off the row", () => {
    expect(toolbarStatus(branchRow(), NOW)).toEqual({ working: "working 12m", stat: "+42 −7" });
    expect(toolbarStatus(issueRow(), NOW)).toEqual({ working: "", stat: "" });
    expect(toolbarStatus(null, NOW)).toEqual({ working: "", stat: "" });
  });
});

describe("what the toolbar says you are standing in", () => {
  const feed = { items: [branchRow(), issueRow()], projects };

  it("names the project and the branch from the route, with the feed's words", () => {
    const identity = toolbarIdentity({ name: "branch", projectId: "p1", branch: "build/login" }, feed);
    expect(identity).toMatchObject({ projectId: "p1", project: "relaydb", kind: "branch", label: "build/login" });
    expect(identity.row).toBe(feed.items[0]);
  });

  it("names an issue by its title", () => {
    const identity = toolbarIdentity({ name: "issue", projectId: "p1", id: "plan-1" }, feed);
    expect(identity).toMatchObject({ kind: "issue", label: "Add a health endpoint", project: "relaydb" });
  });

  it("still names a branch the feed has not caught up with", () => {
    const identity = toolbarIdentity({ name: "branch", projectId: "p1", branch: "build/fresh" }, feed);
    expect(identity).toMatchObject({ label: "build/fresh", project: "relaydb", row: null });
  });

  it("names nothing on a route that is not a work item", () => {
    expect(toolbarIdentity({ name: "inbox" }, feed)).toMatchObject({ kind: null, label: "" });
  });
});

describe("the project selector's menu", () => {
  const items = [branchRow(), issueRow(), branchRow({ project_id: "p2", branch: "build/spike", resume_at: ago(10) })];

  it("carries the projects, and marks the one the toolbar is scoped to", () => {
    expect(projectMenuModel({ projects, projectId: "p1" }).map((project) => [project.name, project.current])).toEqual([
      ["relaydb", true],
      ["mascot", false],
    ]);
  });

  it("carries no work — that is the other menu's list", () => {
    const menu = projectMenuModel({ items, projects, projectId: "p1" });
    expect(menu.every((project) => projects.some((known) => known.id === project.id))).toBe(true);
    expect(menu.map((project) => project.name)).not.toContain("build/login");
  });

  it("filters by project name", () => {
    expect(projectMenuModel({ projects, projectId: "p1", query: "masc" }).map((project) => project.name)).toEqual(["mascot"]);
    // A branch's letters name no project, so the project menu comes back empty.
    expect(projectMenuModel({ projects, projectId: "p1", query: "login" })).toEqual([]);
  });

  it("falls back to the id for a project with no name", () => {
    expect(projectMenuModel({ projects: [{ id: "p9" }], projectId: "p9" })).toEqual([{ id: "p9", name: "p9", current: true }]);
  });
});

describe("the branch-and-issue selector's menu", () => {
  const items = [branchRow(), issueRow(), branchRow({ project_id: "p2", branch: "build/spike", resume_at: ago(10) })];

  it("carries the work inside the scoped project, and no other project's", () => {
    const work = workMenuModel({ items, projectId: "p1" });
    expect(work.map((entry) => entry.label)).toEqual(["Add a health endpoint", "build/login"]);
    expect(work[1].route).toEqual({ name: "branch", projectId: "p1", branch: "build/login", tab: "changes" });
    expect(work[0].route).toEqual({ name: "issue", projectId: "p1", id: "plan-1" });
  });

  it("follows the scope to another project", () => {
    expect(workMenuModel({ items, projectId: "p2" }).map((entry) => entry.label)).toEqual(["build/spike"]);
  });

  it("filters by what the work is called", () => {
    expect(workMenuModel({ items, projectId: "p1", query: "login" }).map((entry) => entry.label)).toEqual(["build/login"]);
    // An issue is findable by its title, a branch by its letters.
    expect(workMenuModel({ items, projectId: "p1", query: "health" }).map((entry) => entry.kind)).toEqual(["issue"]);
    expect(workMenuModel({ items, projectId: "p1", query: "blgn" }).map((entry) => entry.label)).toEqual(["build/login"]);
    // A project's name is not a work item's, so it filters the work away.
    expect(workMenuModel({ items, projectId: "p1", query: "mascot" })).toEqual([]);
  });

  it("leaves out what no URL can name", () => {
    const detached = branchRow({ branch: null, worktree_id: "wt-9" });
    expect(workMenuModel({ items: [detached, issueRow()], projectId: "p1" }).map((entry) => entry.kind)).toEqual(["issue"]);
  });

  it("has nothing to offer before a project is scoped", () => {
    expect(workMenuModel({ items })).toEqual([]);
  });
});

describe("the branch a typed name becomes", () => {
  it("mirrors the daemon's slug rules", () => {
    expect(branchNamePreview("Mascot Model Spike!")).toBe("build/mascot-model-spike");
    expect(branchNamePreview("  ")).toBe("");
    expect(branchNamePreview("***")).toBe("");
  });
});
