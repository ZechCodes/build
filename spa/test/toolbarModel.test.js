// The view-area toolbar's pure model: the identity it prints, the one menu both
// selectors open, and the status its right side reports.

import { describe, it, expect } from "vitest";
import {
  branchNamePreview,
  humanDuration,
  statText,
  toolbarIdentity,
  toolbarMenuModel,
  toolbarStatus,
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

describe("the one menu both selectors open", () => {
  const items = [branchRow(), issueRow(), branchRow({ project_id: "p2", branch: "build/spike", resume_at: ago(10) })];

  it("carries every project and the work inside the scoped one", () => {
    const menu = toolbarMenuModel({ items, projects, projectId: "p1" });
    expect(menu.projects.map((p) => [p.name, p.current])).toEqual([
      ["relaydb", true],
      ["mascot", false],
    ]);
    expect(menu.work.map((entry) => entry.label)).toEqual(["Add a health endpoint", "build/login"]);
    expect(menu.work[1].route).toEqual({ name: "branch", projectId: "p1", branch: "build/login", tab: "changes" });
    expect(menu.work[0].route).toEqual({ name: "issue", projectId: "p1", id: "plan-1" });
  });

  it("filters both halves with one query", () => {
    expect(toolbarMenuModel({ items, projects, projectId: "p1", query: "masc" }).projects.map((p) => p.name)).toEqual(["mascot"]);
    expect(toolbarMenuModel({ items, projects, projectId: "p1", query: "login" }).work.map((w) => w.label)).toEqual(["build/login"]);
    // An issue is findable by its title, a branch by its letters.
    expect(toolbarMenuModel({ items, projects, projectId: "p1", query: "health" }).work.map((w) => w.kind)).toEqual(["issue"]);
    expect(toolbarMenuModel({ items, projects, projectId: "p1", query: "blgn" }).work.map((w) => w.label)).toEqual(["build/login"]);
  });

  it("leaves out what no URL can name", () => {
    const detached = branchRow({ branch: null, worktree_id: "wt-9" });
    const menu = toolbarMenuModel({ items: [detached, issueRow()], projects, projectId: "p1" });
    expect(menu.work.map((entry) => entry.kind)).toEqual(["issue"]);
  });
});

describe("the branch a typed name becomes", () => {
  it("mirrors the daemon's slug rules", () => {
    expect(branchNamePreview("Mascot Model Spike!")).toBe("build/mascot-model-spike");
    expect(branchNamePreview("  ")).toBe("");
    expect(branchNamePreview("***")).toBe("");
  });
});
