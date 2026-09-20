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

const feedItems = [];
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    fn({ items: feedItems, plans: [], runs: [], externalWorktrees: [], projects: [], workspaces: [], devices: {} });
    return () => {};
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: async () => {},
  deliverFeed: () => {},
  joinFeed: () => {},
  dropFeedDevice: () => {},
}));

// The tracker's issue surface is its own file with its own suite; what this
// file is about is the shell around it.
vi.mock("../src/core/trackerIssuePage.js", () => ({
  mountIssuePage: () => ({ feedMoved: () => {}, dispose: () => {} }),
}));

const { App, render, unmountView } = await import("../src/app.js");
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

const strip = () => document.querySelector("#agent-rail .rail-strip");
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
    it(`gives ${place} the bubble strip`, async () => {
      await visit(route);
      // The whole of Zech's report: the strip is there, on every page, at every
      // width. An issue page that mounts no rail is what this catches.
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
    await visit(PLACES["project (issues tab)"]);
    // Same conversation, different page: the tabs are the shell's and the rail
    // is the shell's, so only #root changed.
    expect(strip()).toBe(held);
  });

  it("keeps the strip standing between an issue's legacy URL and the tracker's", async () => {
    await visit(PLACES["issue (tracker)"]);
    const held = strip();
    expect(held).toBeTruthy();
    await visit(PLACES["issue (legacy)"]);
    expect(strip()).toBe(held);
  });

  it("keeps the strip standing as a workspace changes the directory it is open on", async () => {
    await visit(PLACES.workspace);
    const held = strip();
    expect(held).toBeTruthy();
    await visit({ ...PLACES.workspace, sourceId: "s-1", tab: "files" });
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
