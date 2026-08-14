// @vitest-environment jsdom
// The universal three-panel shell: the regions exist, the grid puts them where
// the architecture says, and every route paints into the middle one.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// jsdom gives import.meta.url an http origin, so the sources are read from the
// package root (vitest's cwd), as the other jsdom suites do.
const indexSource = readFileSync(resolve("index.html"), "utf8");
const shellCss = readFileSync(resolve("src/styles/shell.css"), "utf8");
const bodyHtml = indexSource.match(/<body>([\s\S]*)<\/body>/)[1];

// The feed the legacy-URL holding screen resolves against.
const feedItems = [
  { kind: "branch", project_id: "p-1", branch: "build/login", run_id: "run-1", worktree_id: "wt-1" },
];
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    fn({ items: feedItems, plans: [], runs: [], externalWorktrees: [], projects: [], primaryChanges: [] });
    return () => {};
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: () => {},
  primaryRunIdFor: () => null,
}));

const { App, render } = await import("../src/app.js");
const { railStartsCollapsed } = await import("../src/core/inboxShell.js");
const { consoleBarHtml } = await import("../src/core/consoleRegion.js");

const rpc = (method) => {
  if (method === "branch.get") return { kind: "branch", branch: "build/login", state: "building", stat: "+4 −1" };
  if (method === "issue.stages") return { stages: [{ stage_id: "s1", title: "First half", state: "implemented" }] };
  return {};
};

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const root = () => document.getElementById("root");

beforeEach(() => {
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  location.hash = "";
  App.gated = false;
  App.call = vi.fn(async (method, params) => rpc(method, params));
  App.poll = null;
  App.viewDispose = null;
});

afterEach(() => {
  if (App.poll) clearInterval(App.poll);
  App.poll = null;
  App.viewDispose = null;
});

describe("the shell's markup", () => {
  it("carries the three panels, the console slot and the chrome that outlives views", () => {
    for (const id of ["shell", "inbox-rail", "inbox-list", "view", "toolbar", "view-body", "root", "agent-rail", "console-region"]) {
      expect([id, !!document.getElementById(id)]).toEqual([id, true]);
    }
    // The banners, the sheet scrim and the device picker survive the rebuild.
    for (const id of ["offbar", "verbar", "scrim", "sheet", "devpick"]) {
      expect([id, !!document.getElementById(id)]).toEqual([id, true]);
    }
    // Account lives at the foot of the inbox rail.
    expect(document.querySelector("#inbox-rail .inbox-foot #nav-account")).toBeTruthy();
    // The retired sidebar model is gone.
    expect(document.getElementById("sidebar")).toBeNull();
    expect(document.getElementById("side-open")).toBeNull();
    expect(document.getElementById("fab")).toBeNull();
  });

  it("nests the agent rail inside the view body, below the toolbar", () => {
    const rail = document.getElementById("agent-rail");
    expect(rail.parentElement.id).toBe("view-body");
    expect(document.getElementById("root").parentElement.id).toBe("view-body");
    // The toolbar is the view column's own row, above the body the rail rides.
    const view = document.getElementById("view");
    const rows = [...view.children].map((child) => child.id);
    expect(rows).toEqual(["toolbar", "view-body", "console-region"]);
  });
});

describe("the shell's grid", () => {
  it("lays the rail beside the view column, and the view column in three rows", () => {
    expect(shellCss).toMatch(/#shell \{[^}]*display:grid/);
    expect(shellCss).toMatch(/#shell \{[^}]*grid-template-columns:auto minmax\(0, 1fr\)/);
    expect(shellCss).toMatch(/#view \{[^}]*grid-template-rows:auto minmax\(0, 1fr\) auto/);
    expect(shellCss).toMatch(/#view-body \{[^}]*grid-template-columns:minmax\(0, 1fr\) auto/);
  });

  it("gives the gate the whole frame", () => {
    const gated = shellCss.match(/body\.gated[^{]*\{[^}]*\}/)[0];
    for (const region of ["#inbox-rail", "#inbox-open", "#toolbar", "#agent-rail", "#console-region"]) {
      expect([region, gated.includes(region)]).toEqual([region, true]);
    }
  });

  it("overlays the inbox on a narrow viewport, the way the rail it replaces did", () => {
    const narrow = shellCss.match(/@media \(max-width: 900px\) \{[\s\S]*?\n\}/)[0];
    expect(narrow).toMatch(/#inbox-rail \{[^}]*position:fixed/);
    expect(narrow).toMatch(/#inbox-scrim/);
  });
});

describe("the inbox rail's docked state", () => {
  it("starts away only where there is no room for it, and remembers a choice", () => {
    expect(railStartsCollapsed(null, 1400)).toBe(false);
    expect(railStartsCollapsed(null, 700)).toBe(true);
    expect(railStartsCollapsed("1", 1400)).toBe(true);
    expect(railStartsCollapsed("", 700)).toBe(false);
  });
});

describe("render dispatch", () => {
  it("lands the inbox route on the inbox surface", async () => {
    App.route = { name: "inbox" };
    render();
    await flush();
    expect(root().querySelector(".shell-stub")).toBeTruthy();
  });

  it("mounts the branch surface with its two tabs and the console slot", async () => {
    App.route = { name: "branch", projectId: "p-1", branch: "build/login", tab: "changes" };
    render();
    await flush();
    const tabs = [...root().querySelectorAll("#branch-tabs [data-tab]")].map((cell) => cell.dataset.tab);
    expect(tabs).toEqual(["changes", "files"]);
    expect(root().classList.contains("surface")).toBe(true);
    // The console is reserved and shut.
    const bar = document.querySelector("#console-region .console-bar");
    expect(bar).toBeTruthy();
    expect(bar.getAttribute("aria-expanded")).toBe("false");
    // The row is tabs and nothing else: the identity and the status are the
    // toolbar's (core/toolbar.js), and the project cluster is gone.
    expect(root().querySelector("#branch-status")).toBeNull();
    expect(root().querySelector(".tabs-right")).toBeNull();
    expect(root().querySelector(".tback")).toBeNull();
  });

  it("navigates between the branch tabs by URL", async () => {
    App.route = { name: "branch", projectId: "p-1", branch: "build/login", tab: "changes" };
    render();
    await flush();
    root().querySelector('#branch-tabs [data-tab="files"]').click();
    expect(location.hash).toBe("#/project/p-1/branch/build%2Flogin/files");
  });

  it("mounts the issue surface as two columns, no tabs", async () => {
    App.route = { name: "issue", projectId: "p-1", id: "i-1" };
    render();
    await flush();
    expect(root().querySelector(".issue-cols")).toBeTruthy();
    expect(root().querySelector("#branch-tabs")).toBeNull();
    expect(root().querySelector('[data-stage="s1"]').textContent).toContain("First half");
  });

  it("keeps the account pages on the reading column", async () => {
    App.route = { name: "account", page: "archive" };
    render();
    await flush();
    expect(root().className).toBe("");
    expect(root().textContent).toContain("Archive");
  });

  it("rewrites a legacy URL to the work item the feed says it is", async () => {
    App.route = { name: "resolve", kind: "run", id: "run-1", tab: "files" };
    render();
    await flush();
    expect(location.hash).toBe("#/project/p-1/branch/build%2Flogin/files");
  });

  it("lands a legacy URL nothing carries on the inbox", async () => {
    App.route = { name: "resolve", kind: "run", id: "run-gone", tab: "changes" };
    render();
    await flush();
    expect(location.hash).toBe("#/inbox");
  });

  it("hands each view a clean #root and runs the outgoing view's teardown", async () => {
    App.route = { name: "branch", projectId: "p-1", branch: "build/login", tab: "changes" };
    render();
    await flush();
    expect(document.querySelector("#console-region .console-bar")).toBeTruthy();
    App.route = { name: "account", page: "settings" };
    render();
    // The branch view's teardown clears the console region it mounted.
    expect(document.getElementById("console-region").innerHTML).toBe("");
    expect(App.poll).toBeNull();
  });
});

describe("the console slot", () => {
  it("is a shut bar until the console lands", () => {
    expect(consoleBarHtml()).toContain('aria-expanded="false"');
    expect(consoleBarHtml()).toContain("Console");
  });
});
