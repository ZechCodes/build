// @vitest-environment jsdom
// The universal three-panel shell: the regions exist, the grid puts them where
// the architecture says, and every route paints into the middle one.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/** The one bridge this file's device answers through: a test that hands over
 *  a new `call` is that bridge answering differently, not another machine. */
const bridge = { call: null };

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
  dropFeedDevice: () => {},
}));

const { App, render } = await import("../src/app.js");
const { adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const { railStartsCollapsed } = await import("../src/core/inboxShell.js");
const { consoleHeadHtml } = await import("../src/core/console.js");

const rpc = (method) => {
  if (method === "branch.get")
    return { kind: "branch", branch: "build/login", worktree_id: "wt-1", state: "building", stat: "+4 −1" };
  if (method === "issue.get")
    return {
      issue_id: "i-1",
      project_id: "p-1",
      goal: "Rebuild",
      state: "plan_review",
      docs_available: false,
      stages: [{ id: "s1", state: "planned" }],
      implementation_lineage: [],
      thread: { items: [] },
    };
  if (method === "issue.stages")
    return { stages: [{ id: "s1", title: "First half", state: "planned", approval: "planned", execution: "pending" }] };
  return {};
};

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const root = () => document.getElementById("root");

beforeEach(() => {
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  location.hash = "";
  App.gated = false;
  bridge.call = vi.fn(async (method, params) => rpc(method, params));
  App.poll = null;
  App.viewDispose = null;
  App.devices = [{ id: "dev-1", name: "This device", status: "online" }];
  App.selectedDeviceId = "dev-1";
});

/** A work route is about one machine: the view reads its caller off that
 *  device's context, so a surface case opens the device its route names. */
const openDevice = () =>
  adoptDeviceSession({
    deviceId: "dev-1",
    call: (...args) => bridge.call(...args),
    close: () => {},
    peer: () => {},
    onCarrier: () => {},
  });

afterEach(() => {
  App.viewDispose?.();
  if (App.poll) clearInterval(App.poll);
  App.poll = null;
  App.viewDispose = null;
  resetDeviceContexts();
});

describe("the shell's markup", () => {
  it("carries the three panels, the console slot and the chrome that outlives views", () => {
    for (const id of ["shell", "inbox-rail", "inbox-list", "view", "toolbar", "view-body", "branch-tabs", "root", "agent-rail", "console-region"]) {
      expect([id, !!document.getElementById(id)]).toEqual([id, true]);
    }
    // The banners, the sheet scrim and the device picker survive the rebuild.
    for (const id of ["verbar", "scrim", "sheet", "devpick"]) {
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
    expect(shellCss).toMatch(/#shell \{[^}]*grid-template-columns:var\(--inbox-space\) minmax\(0, 1fr\)/);
    expect(shellCss).toMatch(/#view \{[^}]*grid-template-rows:auto minmax\(0, 1fr\) auto/);
    expect(shellCss).toMatch(/#view-body \{[^}]*grid-template-columns:auto minmax\(0, 1fr\) auto/);
  });

  it("pins each panel to its own track, so the view column never lands in the rail's", () => {
    // The rail leaves the flow twice — hidden when it is collapsed, fixed below
    // the stacking width — and auto-placement then drops the view column into
    // the `auto` track, where it sizes to its content instead of filling the
    // frame. Naming both tracks is what keeps the shell viewport-wide with the
    // rail away.
    expect(shellCss).toMatch(/#inbox-rail \{[^}]*grid-column:1/);
    expect(shellCss).toMatch(/#view \{[^}]*grid-column:2/);
  });

  it("gives the view column one definite track, so the rail cannot be pushed off the frame", () => {
    // #view is a grid with named rows and, without this, an implicit `auto`
    // column — which sizes to the widest child's max-content. The toolbar is
    // that child (its 42% cap on each name is not a constraint an intrinsic
    // sizing pass can honour), so on a phone the whole view body rendered
    // wider than the viewport and the agent rail sat past the right edge,
    // clipped by #view's own overflow with no way to reach it.
    expect(shellCss).toMatch(/#view \{[^}]*grid-template-columns:minmax\(0, 1fr\)/);
  });

  it("gives the gate the whole frame", () => {
    const hidden = shellCss.match(/body\.gated[^{]*\{[^}]*display:none[^}]*\}/g).join("\n");
    for (const region of ["#inbox-open", "#toolbar", "#agent-rail", "#console-region"]) {
      expect([region, hidden.includes(region)]).toEqual([region, true]);
    }
    expect(shellCss).toMatch(/body\.gated \{[^}]*--inbox-space:0px/);
    expect(shellCss).toMatch(/body\.gated #inbox-rail \{[^}]*display:none/);
  });

  it("overlays the inbox on a narrow viewport, the way the rail it replaces did", () => {
    const narrow = shellCss.match(/@media \(max-width: 900px\) \{[\s\S]*?\n\}/)[0];
    expect(narrow).toMatch(/#inbox-rail \{[^}]*position:absolute/);
    expect(narrow).toMatch(/body \{[^}]*--inbox-space:0px/);
    expect(narrow).toMatch(/#inbox-scrim/);
  });

  it("overlays the conversation on a phone without covering the toolbar", () => {
    const narrow = shellCss.match(/@media \(max-width: 760px\) \{[\s\S]*?\n\}/)[0];
    const panel = narrow.match(/\.rail-panel \{[^}]*\}/)[0];
    // Laid over the view column, not over the page: the toolbar above it keeps
    // saying which project and branch the agent is working in, the one fact a
    // phone cannot afford to lose.
    expect(panel).toMatch(/position:absolute/);
    expect(panel).toMatch(/top:calc\(var\(--toolbar-h\) \+ 1px\)/);
    expect(panel).toMatch(/right:var\(--agent-strip\)/);
    expect(panel).not.toMatch(/top:0/);
  });
});

describe("the inbox rail's docked state", () => {
  it("starts away only where there is no room for it, and remembers a choice", () => {
    expect(railStartsCollapsed(null, 1400)).toBe(false);
    expect(railStartsCollapsed(null, 700)).toBe(true);
    expect(railStartsCollapsed("1", 1400)).toBe(true);
    expect(railStartsCollapsed("", 700)).toBe(true);
    expect(railStartsCollapsed("", 900)).toBe(true);
  });
});

describe("render dispatch", () => {
  it("lands the inbox route on the inbox surface", async () => {
    App.route = { name: "inbox" };
    render();
    await flush();
    expect(root().querySelector(".shell-stub")).toBeTruthy();
  });

  it("keeps a legacy branch conversation reachable, with its two tabs and the console slot", async () => {
    openDevice();
    App.route = { name: "branch", deviceId: "dev-1", projectId: "p-1", branch: "build/login", tab: "changes" };
    render();
    await flush();
    const tabs = [...document.querySelectorAll("#branch-tabs [data-tab]")].map((cell) => cell.dataset.tab);
    expect(tabs).toEqual(["files", "changes"]);
    expect(document.querySelector("#agent-rail .rail-strip")).not.toBeNull();
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
    openDevice();
    App.route = { name: "branch", deviceId: "dev-1", projectId: "p-1", branch: "build/login", tab: "changes" };
    render();
    await flush();
    document.querySelector('#branch-tabs [data-tab="files"]').click();
    // The link the tab writes keeps the machine the surface is standing on.
    expect(location.hash).toBe("#/device/dev-1/project/p-1/branch/build%2Flogin/files");
  });

  it("keeps a legacy issue transcript reachable, as two columns with no tabs", async () => {
    openDevice();
    App.route = { name: "issue", deviceId: "dev-1", projectId: "p-1", id: "i-1" };
    render();
    await flush();
    expect(document.querySelector("#agent-rail .rail-strip")).not.toBeNull();
    expect(root().querySelector(".ivsplit")).toBeTruthy();
    expect(document.querySelector("#branch-tabs").children).toHaveLength(0);
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
    openDevice();
    App.route = { name: "branch", deviceId: "dev-1", projectId: "p-1", branch: "build/login", tab: "changes" };
    render();
    await flush();
    expect(document.querySelector("#branch-tabs").children.length).toBeGreaterThan(0);
    App.route = { name: "account", page: "settings" };
    render();
    expect(document.getElementById("console-region").innerHTML).toBe("");
    expect(App.poll).toBeNull();
  });
});

describe("the console", () => {
  it("renders its head into the region the skeleton gives it", () => {
    const region = document.getElementById("console-region");
    region.innerHTML = `<div class="console"><div class="console-head">${consoleHeadHtml("collapsed")}</div></div>`;
    expect(region.querySelector(".console-head #console-toggle")).toBeTruthy();
    expect(region.querySelector(".console-head .console-tabs")).toBeTruthy();
  });

  it("takes the bottom row of the view column, and overlays it at full size", () => {
    expect(shellCss).toMatch(/#console-region \{[^}]*height:var\(--console-bar\)/);
    expect(shellCss).toMatch(/#console-region\[data-size="half"\] \{[^}]*height:var\(--console-half\)/);
    const full = shellCss.match(/#console-region\[data-size="full"\] \{[^}]*\}/)[0];
    expect(full).toMatch(/position:absolute/);
    // …and never over the toolbar or the bubble strip: where you are standing
    // and what every agent is doing stay legible under an open console.
    expect(full).toMatch(/top:calc\(var\(--toolbar-h\) \+ 1px\)/);
    expect(full).toMatch(/right:var\(--agent-strip\)/);
  });
});
