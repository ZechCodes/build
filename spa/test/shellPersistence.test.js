// @vitest-environment jsdom
// One shell for every page.
//
// Zech, on his phone, opening an issue: "Chat bubble goes away as does the
// entire bar except the status indicator." The issue page mounted no rail, and
// it could because each page mounted its own. This walks the routes and holds
// the shell to the rule: the regions are the same DOM nodes across every
// navigation, every place with a conversation shows the bubble strip, and a
// page swapping inside the shell does not take the strip down with it.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bridge = { call: null };

const indexSource = readFileSync(resolve("index.html"), "utf8");
const bodyHtml = indexSource.match(/<body>([\s\S]*)<\/body>/)[1];

// A branch row, so the legacy branch page has a checkout to stand on rather
// than painting "no checkout in this project carries build/login".
const feedItems = [
  { kind: "branch", deviceId: "dev-1", project_id: "p-1", branch: "build/login", run_id: "run-1", worktree_id: "wt-1", state: "building" },
];
// …and the project, so its page has a row to list rather than its empty state.
const feedProject = { id: "p-1", project_id: "p-1", name: "Login", deviceId: "dev-1", projectKey: "dev-1/p-1" };
// The machine's checkout list, as a pass leaves it. The workspace page stands
// on the RECORD rather than on a read (views/workspaceView.js), and it is found
// through the per-device slice — so the snapshot has to carry a `devices` entry
// for dev-1, not just the flat lists. Without one the workspace page sat at
// "loading…" for the whole of this file and the directory case below changed
// no directory.
const feedWorkspace = {
  id: "w-1",
  workspace_id: "w-1",
  name: "Login",
  status: "ready",
  deviceId: "dev-1",
  projectKey: "dev-1/p-1",
  workspaceKey: "dev-1/w-1",
  updated_at: "2026-01-01T00:00:00Z",
  project_id: "p-1",
  directories: [
    { source_id: "s-1", name: "Build", is_git: true },
    { source_id: "s-2", name: "Assets", is_git: true },
  ],
};
const feedSnapshot = () => ({
  items: feedItems, plans: [], runs: [], externalWorktrees: [], projects: [feedProject], workspaces: [feedWorkspace],
  devices: { "dev-1": { items: feedItems, projects: [feedProject], workspaces: [feedWorkspace] } },
});
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    fn(feedSnapshot());
    return () => {};
  },
  startFeed: () => {},
  stopFeed: () => {},
  // An ARRAY, which is what the real one answers with — refreshFeed is a
  // Promise.all over the devices it synced, and views/workspaceView.js takes
  // the first element straight out of it (`const [passed] = await
  // refreshFeed(...)`). Answering `undefined` made that destructure throw on
  // every workspace visit, which was the seven unhandled rejections the suite
  // reported and this file caused (#32). `false` stands for "a pass ran and
  // established nothing", leaving the page waiting on the records rather than
  // calling the workspace unknown.
  refreshFeed: async () => [false],
  deliverFeed: () => {},
  joinFeed: () => {},
  dropFeedDevice: () => {},
}));

// The tracker's own surfaces have their own files and their own suites; what
// this file is about is the shell around them.
vi.mock("../src/core/trackerIssuePage.js", () => ({
  mountIssuePage: () => ({ feedMoved: () => {}, dispose: () => {} }),
}));
vi.mock("../src/core/trackerIssuesPane.js", () => ({
  // It draws the frame the real pane draws. A stand-in that renders nothing
  // makes "did the page stand up" unanswerable, which is the one question this
  // file is built on (#36) — the mocked pane would have left the project's
  // Issues tab looking exactly like a tab that never mounted.
  mountIssuesPane: (host) => {
    host.innerHTML = '<div class="issue-head"></div>';
    return { feedMoved: () => {}, dispose: () => {} };
  },
}));

const { App, go, render, unmountView } = await import("../src/app.js");
const { adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const { stopShell } = await import("../src/core/shell.js");

const rpc = (method) => {
  if (method === "project.ensure_conversation") return { entity_id: "proj-conv-1" };
  if (method === "branch.get") return { kind: "branch", branch: "build/login", worktree_id: "wt-1", state: "building" };
  if (method === "issue.get")
    return {
      issue_id: "i-1", project_id: "p-1", goal: "Rebuild", state: "plan_review",
      docs_available: false, stages: [{ id: "s1", state: "planned" }],
      implementation_lineage: [], thread: { items: [] },
    };
  if (method === "issue.stages") return { stages: [{ id: "s1", title: "First half", state: "planned", approval: "planned", execution: "pending" }] };
  if (method === "workspace.get")
    return { workspace_id: "w-1", project_id: "p-1", name: "Login", status: "ready", directories: [{ source_id: "s-1", name: "Build", is_git: true, status: "ready" }] };
  if (method === "workspace.ensure_conversation") return { entity_id: "ws-conv-1" };
  return {};
};

const flush = () => new Promise((done) => setTimeout(done, 0));

/** Every route that is a place with a conversation, and what the reader calls it. */
const PLACES = {
  project: { name: "project", deviceId: "dev-1", projectId: "p-1" },
  "project (issues tab)": { name: "project", deviceId: "dev-1", projectId: "p-1", tab: "issues" },
  "issue (tracker)": { name: "trackerIssue", deviceId: "dev-1", projectId: "p-1", issueId: "i-1" },
  workspace: { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "w-1" },
  // #29: the workspace's own issues, and one of them open. Both are still
  // workspace routes, which is what keeps the rail standing across them.
  "workspace (issues tab)": { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "w-1", tab: "issues" },
  "workspace (issue open)": { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "w-1", tab: "issues", issueId: "i-1" },
  "issue (legacy)": { name: "issue", deviceId: "dev-1", projectId: "p-1", id: "i-1" },
  "branch (legacy)": { name: "branch", deviceId: "dev-1", projectId: "p-1", branch: "build/login", tab: "changes" },
};

const openDevice = () =>
  adoptDeviceSession({
    deviceId: "dev-1",
    call: (...args) => bridge.call(...args),
    close: () => {},
    peer: () => {},
    onCarrier: () => {},
  });

/** Go to a route the way the app does, and let the reads that paint it land. */
const visit = async (route) => {
  App.route = route;
  render();
  await flush();
  await flush();
};

/**
 * What proves the PAGE's own body stood up, per place.
 *
 * Asserted BEFORE anything about the shell, because a shell case standing over
 * a page that never mounted tests half of what its name claims — and passes,
 * the rail being the shell's rather than the page's. That is exactly how the
 * workspace case here ran for a while against a `loading…` frame (#32/#36).
 */
const PAGE_CONTENT = {
  project: "[data-workspace]",
  "project (issues tab)": ".issue-head",
  // The issue surface itself is core/trackerIssuePage.js, mocked at the top of
  // this file because it has a suite of its own — so its host pane is what
  // there is to see, and seeing it is what says the route host ran.
  "issue (tracker)": "#issue-pane",
  workspace: ".workspace-gitpane",
  // #29. Both hosts are drawn by core/workspaceIssuesTab.js rather than by the
  // tracker inside them, which is what makes them answerable with the pane and
  // the page both mocked: seeing the host is what says the TAB ran.
  "workspace (issues tab)": ".workspace-issues-pane",
  "workspace (issue open)": ".issue-surface",
  "issue (legacy)": ".ivsplit",
  "branch (legacy)": ".gitpane",
};

const pageContent = (place) => document.querySelector(`#root ${PAGE_CONTENT[place]}`);
const strip = () => document.querySelector("#agent-rail .rail-strip");
const separators = () => document.querySelectorAll("#agent-rail .rail-sep");
const bubbleIds = () => [...document.querySelectorAll("#agent-rail .rail-strip [data-agent]")].map((b) => b.dataset.agent);
const regions = () => ["shell", "toolbar", "view-body", "root", "agent-rail", "console-region"].map((id) => document.getElementById(id));

beforeEach(() => {
  localStorage.clear();
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  location.hash = "";
  App.gated = false;
  App.poll = null;
  App.viewDispose = null;
  App.devices = [{ id: "dev-1", name: "This device", status: "online" }];
  App.selectedDeviceId = "dev-1";
  bridge.call = vi.fn(async (method, params) => rpc(method, params));
  openDevice();
});

afterEach(() => {
  // Leave the way the reader would: a case that ends with a modal open would
  // otherwise hand the next one an app that thinks settings are already up.
  App.route = { name: "inbox" };
  render();
  // …and then down to nothing, so the next case starts with an empty #root and
  // an app that knows it: this file replaces the document body between cases,
  // which the running app never does.
  unmountView();
  App.poll?.dispose?.();
  App.poll = null;
  App.viewDispose = null;
  stopShell();
  resetDeviceContexts();
});

describe("every place stands in the same shell", () => {
  for (const [place, route] of Object.entries(PLACES)) {
    it(`stands ${place} up, and gives it the bubble strip`, async () => {
      await visit(route);
      // The page first: a strip beside an empty frame proves nothing.
      expect([place, Boolean(pageContent(place))]).toEqual([place, true]);
      // Then Zech's report: the strip is there, on every page, at every width.
      // An issue page that mounts no rail is what this catches.
      expect([place, Boolean(strip())]).toEqual([place, true]);
    });
  }

  it("leaves the regions themselves standing across every navigation", async () => {
    await visit(PLACES.project);
    const before = regions();
    for (const route of Object.values(PLACES)) await visit(route);
    await visit({ name: "inbox" });
    await visit(PLACES.workspace);
    // Not "present again" — the same nodes. The shell is mounted once and the
    // page swaps inside it; a region that were rebuilt would take the toolbar's
    // paint, the console's sessions and the rail's scroll with it.
    expect(regions()).toEqual(before);
    expect(before.every(Boolean)).toBe(true);
  });
});

describe("a page swapping inside the shell", () => {
  it("keeps the strip standing across a project's Workspaces and Issues tabs", async () => {
    await visit(PLACES.project);
    const held = strip();
    expect(held).toBeTruthy();
    expect(pageContent("project")).toBeTruthy();
    await visit(PLACES["project (issues tab)"]);
    // The page really swapped: the workspaces went, the issues came.
    expect(pageContent("project (issues tab)")).toBeTruthy();
    expect(pageContent("project")).toBeNull();
    // Same conversation, different page: the tabs are the shell's and the rail
    // is the shell's, so only #root changed.
    expect(strip()).toBe(held);
  });

  // #29. The reader opens an issue, works on it, and talks to the agent
  // holding it — which only works if the agent is still there. The rail is
  // keyed on the workspace, so every one of these is the same standing.
  it("keeps the WORKSPACE's strip standing across its tabs and one of its issues", async () => {
    await visit(PLACES.workspace);
    const held = strip();
    expect(held).toBeTruthy();
    expect(pageContent("workspace")).toBeTruthy();

    // Each leg asserts the page REALLY swapped before it asserts the strip did
    // not: a marker that is present everywhere proves nothing, and a shell case
    // standing over a page that never mounted is the hole #36 closed.
    await visit(PLACES["workspace (issues tab)"]);
    expect(pageContent("workspace (issues tab)")).toBeTruthy();
    expect(pageContent("workspace")).toBeNull();
    expect(strip()).toBe(held);

    await visit(PLACES["workspace (issue open)"]);
    expect(pageContent("workspace (issue open)")).toBeTruthy();
    expect(pageContent("workspace (issues tab)")).toBeNull();
    expect(strip()).toBe(held);

    // …and back to the checkout, without the bubbles having moved once.
    await visit(PLACES.workspace);
    expect(pageContent("workspace")).toBeTruthy();
    expect(pageContent("workspace (issue open)")).toBeNull();
    expect(strip()).toBe(held);
  });

  it("keeps the strip standing when the Issues tab opens one of its issues", async () => {
    await visit(PLACES["project (issues tab)"]);
    const held = strip();
    expect(held).toBeTruthy();
    await visit(PLACES["issue (tracker)"]);
    // A tracker issue carries no conversation of its own: its page stands on
    // the PROJECT's agent, the same standing the tab under it had. So pressing
    // an issue swaps the page and leaves the bubbles exactly where they were.
    expect(strip()).toBe(held);
  });

  it("stands the legacy issue page on the issue's own conversation instead", async () => {
    // The legacy multi-stage issue is the one issue that does carry one.
    await visit(PLACES["project (issues tab)"]);
    const held = strip();
    await visit(PLACES["issue (legacy)"]);
    expect(strip()).toBeTruthy();
    expect(strip()).not.toBe(held);
  });

  it("keeps the strip standing as a workspace changes the directory it is open on", async () => {
    await visit(PLACES.workspace);
    const held = strip();
    expect(held).toBeTruthy();
    // Only the directory changes — not the tab as well, so what this case is
    // named for is the one thing that moved.
    await visit({ ...PLACES.workspace, sourceId: "s-2" });
    // …and the page under it really moved to the other directory: the app is
    // standing on it, and the page painted a body for it.
    expect(App.route.sourceId).toBe("s-2");
    expect(pageContent("workspace")).toBeTruthy();
    expect(strip()).toBe(held);
  });

  it("stands the rail on the new conversation when the reader moves to another one", async () => {
    await visit(PLACES.project);
    const held = strip();
    await visit(PLACES.workspace);
    // A different work item IS a different conversation: this one must remount.
    expect(strip()).toBeTruthy();
    expect(strip()).not.toBe(held);
  });
});

describe("the places that are not a conversation", () => {
  it("leaves the strip away on the inbox, a capture and a legacy link being looked up", async () => {
    for (const route of [{ name: "inbox" }, { name: "capture", id: "c-1" }, { name: "resolve", kind: "run", id: "run-x" }]) {
      await visit(PLACES.project);
      expect(strip()).toBeTruthy();
      await visit(route);
      // The bar, the regions and the inbox rail are still there; there is just
      // no work item to converse with, and an empty strip says so honestly.
      expect([route.name, Boolean(strip())]).toEqual([route.name, false]);
    }
  });
});

describe("a modal over a page", () => {
  // Settings, the account and a device's settings are configuration, not a
  // place to stand: they open OVER the page the reader was on and close back
  // to it. "Over" has to mean it — a modal that tore the page down would lose
  // the rail's scroll, the console's sessions and every read the page holds,
  // and closing would pay for all of them again.
  it("leaves the page under settings mounted, and closing does not build it again", async () => {
    await visit(PLACES.workspace);
    const page = document.querySelector("#root").firstElementChild;
    const held = strip();
    expect(page).toBeTruthy();
    expect(held).toBeTruthy();

    await visit({ name: "account", page: "settings" });
    expect(document.querySelector('[role="dialog"]')).toBeTruthy();
    expect(document.querySelector("#root").firstElementChild).toBe(page);
    expect(strip()).toBe(held);

    await visit(PLACES.workspace);
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.querySelector("#root").firstElementChild).toBe(page);
    expect(strip()).toBe(held);
  });

  it("keeps the bubbles beside an issue while its device settings are open", async () => {
    await visit(PLACES["issue (tracker)"]);
    const held = strip();
    await visit({ name: "device", id: "dev-1" });
    expect(document.querySelector('[role="dialog"]')).toBeTruthy();
    expect(strip()).toBe(held);
  });

  // A URL may still name a modal for a deep link. Opening one that way has no
  // page under it yet, so the shell stands the reader's landing page up first:
  // closing has somewhere to land, and the frame behind the scrim is never bare.
  it("stands a page up under a settings deep link so closing lands somewhere", async () => {
    await visit({ name: "account", page: "settings" });
    expect(document.querySelector('[role="dialog"]')).toBeTruthy();
    expect(document.querySelector("#root").children.length).toBeGreaterThan(0);
  });
});

describe("what the reader is looking at, while a modal is over the page", () => {
  // #21 has the issue page say which issue is on screen, so the project agent's
  // rail beside it knows. The page sets it from its own read; the router clears
  // it on every navigation. A modal is not a navigation — the reader is still
  // looking at the issue, with settings laid over it — and the page under does
  // not read again on the way back, so a clear there is a clear for good.
  it("keeps the issue named while settings open and close over it", async () => {
    // The context is off until a bridge says it takes one (core/viewingContext).
    App.viewingContext.setEnabled(true);
    await visit(PLACES["issue (tracker)"]);
    App.viewingContext.set({ version: 1, items: [{ kind: "issue", issue_id: "i-1", title: "Rebuild", number: 3 }] });
    expect(App.viewingContext.snapshot()).toBeTruthy();

    await visit({ name: "account", page: "settings" });
    expect(App.viewingContext.snapshot()).toBeTruthy();

    await visit(PLACES["issue (tracker)"]);
    expect(App.viewingContext.snapshot()).toBeTruthy();
  });

  // …and a real navigation still drops it: the reader is looking at something
  // else now, and the agent must not be told about the page they left. Taken
  // through `go`, which is where the router drops it — the `visit` above sets
  // App.route and renders, so it would never reach that code at all.
  it("forgets it when the reader actually goes somewhere else", async () => {
    App.viewingContext.setEnabled(true);
    await visit(PLACES["issue (tracker)"]);
    App.viewingContext.set({ version: 1, items: [{ kind: "issue", issue_id: "i-1", title: "Rebuild", number: 3 }] });
    expect(App.viewingContext.snapshot()).toBeTruthy();

    go(PLACES.workspace);
    expect(App.viewingContext.snapshot()).toBeFalsy();
  });

  it("keeps it when the route taken up is the modal, or the page already under it", async () => {
    App.viewingContext.setEnabled(true);
    await visit(PLACES["issue (tracker)"]);
    const named = { version: 1, items: [{ kind: "issue", issue_id: "i-1", title: "Rebuild", number: 3 }] };

    App.viewingContext.set(named);
    go({ name: "account", page: "settings" }); // the modal going up
    expect(App.viewingContext.snapshot()).toBeTruthy();

    go(PLACES["issue (tracker)"]); // …and closing back onto the page under it
    expect(App.viewingContext.snapshot()).toBeTruthy();
  });
});

describe("the strip on a page standing on the project", () => {
  // Zech, on the project page after the shell roll: two bubbles for the one
  // project agent, one above the separator wearing the agent's unread count in
  // place of the project's initial, and the same agent again below the line.
  //
  // The line exists to separate the project's agent from the agents of the
  // thing you are standing IN. On the project page there is no such thing —
  // the project's conversation is what the page stands on — so asking for the
  // bubble above the line asked for the very same conversation twice.
  it("draws the project's conversation once, with no line", async () => {
    await visit(PLACES.project);
    expect(strip()).toBeTruthy();
    expect(separators()).toHaveLength(0);
    const ids = bubbleIds();
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("draws it once on an issue of the project too, which stands on the same conversation", async () => {
    await visit(PLACES["issue (tracker)"]);
    expect(strip()).toBeTruthy();
    expect(separators()).toHaveLength(0);
    const ids = bubbleIds();
    expect(new Set(ids).size).toBe(ids.length);
  });

  // …and a page standing on something else in the project still gets it: the
  // project's agent is reachable from every workspace in the project, and the
  // line is what says which half of the strip is which.
  it("keeps the project's bubble above the line on a workspace", async () => {
    await visit(PLACES.workspace);
    expect(strip()).toBeTruthy();
    expect(separators()).toHaveLength(1);
  });
});
