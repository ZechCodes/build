// @vitest-environment jsdom
// The view-area toolbar on main's workspace chrome: the sentence it prints, the
// project menu behind its left half, the workspace switcher, and the directory
// tabs of the workspace the route is standing in — every one of them read from
// the machine the route names.

import { describe, it, expect, beforeEach, afterAll, vi } from "vitest";
import { waitFor } from "./waitFor.js";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const NOW = Date.now();
const ago = (seconds) => new Date(NOW - seconds * 1000).toISOString();

let savedFeed = null;
let feed = {
  items: [
    {
      kind: "branch",
      deviceId: "dev-1",
      projectKey: "dev-1/p1",
      project_id: "p1",
      project: "relaydb",
      branch: "build/login",
      title: "Fix the login flow",
      state: "building",
      working: true,
      working_time: { since: ago(750), seconds: 750 },
      stat: { files_changed: 3, insertions: 42, deletions: 7 },
      resume_at: ago(60),
    },
    {
      kind: "issue",
      deviceId: "dev-1",
      projectKey: "dev-1/p1",
      project_id: "p1",
      project: "relaydb",
      branch: null,
      issue_id: "plan-1",
      title: "Add a health endpoint",
      state: "created",
      working: false,
      working_time: null,
      stat: null,
      resume_at: ago(30),
    },
  ],
  projects: [
    { id: "p1", deviceId: "dev-1", projectKey: "dev-1/p1", name: "relaydb", path: "/repos/relaydb" },
    { id: "p2", deviceId: "dev-1", projectKey: "dev-1/p2", name: "mascot" },
  ],
  workspaces: [],
  devices: {},
};
// A Set, matching the real module (core/taskFeed.js): this file's own
// navigation can land the router on a route it has already rendered (the
// hash unchanged), which calls render() straight through rather than via a
// hashchange listener this test never wires up — and that can mount the
// agent rail, which subscribes to the feed too. A single-slot stub would let
// that second subscriber silently steal the toolbar's own.
const subscribers = new Set();
const refreshFeed = vi.fn(async () => {
  subscribers.forEach((fn) => fn(feed));
  return [];
});
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    subscribers.add(fn);
    fn(feed);
    return () => subscribers.delete(fn);
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: (...args) => refreshFeed(...args),
  deliverFeed: () => subscribers.forEach((fn) => fn(feed)),
  dropFeedDevice: () => {},
}));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notifySuccess: () => {} }));
const openCreateWork = vi.fn();
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: (...args) => openCreateWork(...args) }));

const { App } = await import("../src/app.js");
const { clearProjectTabHandler, initToolbar, setProjectTabHandler, stopToolbar, toolbarRouteChanged } = await import("../src/core/toolbar.js");
const { splitDeviceKey } = await import("../src/core/deviceKey.js");
const { routeFromHash } = await import("../src/core/router.js");
const { adoptDeviceSession } = await import("../src/core/deviceContexts.js");
const { rememberDeviceFilter } = await import("../src/core/deviceFilter.js");
const { stampWorkspace } = await import("../src/core/feedMerge.js");
const { readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { uiAddress } = await import("../src/core/localUiState.js");

/** What each machine answers. A workspace read the bar makes is made on the
 *  machine the project it is about lives on, so the two are told apart. */
const workshopCall = vi.fn(async () => ({}));
const laptopCall = vi.fn(async () => ({}));
const openSession = (deviceId, call) => adoptDeviceSession({ deviceId, call, close: () => {}, peer: () => {}, onCarrier: () => {} });

const bar = () => document.querySelector("#toolbar .toolbar");
const menu = () => document.querySelector(".tbmenu");
const names = () => [...bar().querySelectorAll(".tb-name")].map((name) => name.textContent);
const openJump = async (which = "project") => {
  bar().querySelector(`[data-select="${which}"]`).click();
  const list = { project: "projects", workspace: "workspaces", directory: "directories" }[which];
  await waitFor(() => expect(menu()?.dataset.list).toBe(list));
  return menu();
};
const waitMenuList = (list) => waitFor(() => expect(menu()?.dataset.list).toBe(list));
const waitMenuClosed = () => waitFor(() => expect(menu()).toBeNull());
const labels = (selector) => [...menu().querySelectorAll(selector)].map((row) => row.querySelector(".mt").textContent);

/** The workspace the workshop is holding, with two directories in it. */
const payments = {
  id: "ws-1",
  workspace_id: "ws-1",
  project_id: "p1",
  name: "payment-work",
  status: "ready",
  directories: [
    { source_id: "frontend", name: "Frontend", is_git: true },
    { source_id: "assets", name: "Design assets", is_git: false },
  ],
};

/** The workshop's checkouts, as one pass left them in the cache: the bar is a
 *  view over the same records the rail is, and asks no machine for them. */
const workshopHolds = (byProject) => {
  const workspaces = Object.values(byProject)
    .flat()
    .map((workspace) => stampWorkspace(workspace, "dev-1"));
  feed = { ...feed, workspaces, devices: { ...feed.devices, "dev-1": { ...feed.devices?.["dev-1"], workspaces } } };
  subscribers.forEach((fn) => fn(feed));
};

/** Stand on the workshop's payment-work, with its directory tabs hydrated. */
const standOnWorkspace = async () => {
  App.route = { name: "workspace", deviceId: "dev-1", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" };
  toolbarRouteChanged();
  const workspaceName = feed.workspaces?.find((item) => (item.workspace_id || item.id) === "ws-1")?.name || "ws-1";
  await waitFor(() => expect(bar()?.querySelector('[data-select="workspace"] .tb-name')?.textContent).toBe(workspaceName));
};

beforeEach(async () => {
  if (!savedFeed) savedFeed = feed;
  await stopToolbar();
  await wipeCache();
  if (!document.getElementById("shell")) document.body.innerHTML = bodyHtml;
  localStorage.clear();
  App.gated = false;
  App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login", tab: "changes" };
  App.focusComposerOnMount = false;
  App.devices = [
    { id: "dev-1", name: "workshop", status: "online" },
    { id: "dev-2", name: "laptop", status: "online" },
  ];
  openCreateWork.mockClear();
  notifyError.mockClear();
  await rememberDeviceFilter(null);
  workshopCall.mockReset();
  laptopCall.mockReset();
  workshopHolds({ p1: [payments] });
  laptopCall.mockImplementation(async () => ({}));
  openSession("dev-1", workshopCall);
  openSession("dev-2", laptopCall);
  await initToolbar();
  // The cache wipe announces to the app's device watcher asynchronously; give
  // this fixture its known devices after that readback has completed.
  App.devices = [
    { id: "dev-1", name: "workshop", status: "online" },
    { id: "dev-2", name: "laptop", status: "online" },
  ];
  toolbarRouteChanged();
});

afterAll(async () => { await stopToolbar(); });

describe("the sentence the toolbar prints", () => {
  it("names the project, then the branch — the working time and diffstat pin above the agent rail's composer instead", async () => {
    expect(names()).toEqual(["relaydb", "build/login"]);
    expect(document.getElementById("tb-status")).toBeNull();
  });

  it("names an issue by its title", async () => {
    App.route = { name: "issue", deviceId: "dev-1", projectId: "p1", id: "plan-1" };
    toolbarRouteChanged();
    expect(names()).toEqual(["relaydb", "Add a health endpoint"]);
  });

  it("keeps the project selector on a route that is no work item", async () => {
    App.route = { name: "inbox" };
    toolbarRouteChanged();
    expect(names()).toEqual(["relaydb"]);
    expect(bar().querySelector('[data-select="item"]')).toBeNull();
  });
});

// The project's two pages ride the bar after its name, so they are reachable
// from an issue's page too, with the chat open over it on a phone.
describe("the project's pages in the bar", () => {
  const projectTabs = () =>
    [...bar().querySelectorAll("[data-project-tab]")].map((tab) => [tab.textContent, tab.getAttribute("aria-selected")]);
  const standOn = (route) => {
    App.route = route;
    toolbarRouteChanged();
  };

  // Issues first, and the one a route that names no tab is standing on (#46).
  it("follows the project's name with Issues and Workspaces, marking the page you are on", async () => {
    standOn({ name: "project", deviceId: "dev-1", projectId: "p1" });
    expect(names()).toEqual(["relaydb"]);
    expect(projectTabs()).toEqual([["Issues", "true"], ["Workspaces", "false"]]);
    standOn({ name: "project", deviceId: "dev-1", projectId: "p1", tab: "workspaces" });
    expect(projectTabs()).toEqual([["Issues", "false"], ["Workspaces", "true"]]);
  });

  it("keeps them over an issue's page with Issues open, and goes back to the list from either", async () => {
    standOn({ name: "trackerIssue", deviceId: "dev-1", projectId: "p1", issueId: "issue-1" });
    expect(projectTabs()).toEqual([["Issues", "true"], ["Workspaces", "false"]]);
    bar().querySelector('[data-project-tab="issues"]').click();
    // The default tab's own URL: the bare project link (#46).
    expect(location.hash).toBe("#/device/dev-1/project/p1");
    standOn({ name: "trackerIssue", deviceId: "dev-1", projectId: "p1", issueId: "issue-1" });
    bar().querySelector('[data-project-tab="workspaces"]').click();
    expect(location.hash).toBe("#/device/dev-1/project/p1/workspaces");
  });

  // The page is the same page: a navigation would remount the rail beside it.
  it("hands a press to the project page standing under it rather than navigating", async () => {
    standOn({ name: "project", deviceId: "dev-1", projectId: "p1" });
    const hash = location.hash;
    const opened = vi.fn((tab) => {
      App.route = { ...App.route, tab };
    });
    setProjectTabHandler(opened);
    // Pressing the tab the page is NOT on, which is Workspaces now (#46).
    bar().querySelector('[data-project-tab="workspaces"]').click();
    clearProjectTabHandler(opened);
    expect(opened).toHaveBeenCalledWith("workspaces");
    expect(location.hash).toBe(hash);
    expect(projectTabs()).toEqual([["Issues", "false"], ["Workspaces", "true"]]);
  });

  it("draws none over a workspace, whose tabs are its directories", async () => {
    await standOnWorkspace();
    expect(projectTabs()).toEqual([]);
  });
});

// #47. Zech, on the workspace page: "The bar with the workspace name, tabs, and
// icons is workspace scoped. The rail on the left is directory scoped (tabs).
// Issues are workspace scoped so shouldn't be in the left rail. Also the issues
// icon in the workspace bar is too large. Might be better to just show the text
// 'Issues' with a counter bubble."
describe("the workspace's Issues, in the bar", () => {
  const issues = () => bar().querySelector("[data-workspace-issues]");

  // A CSS fact a jsdom case cannot see: the entry sets `display`, which beats
  // the UA's [hidden] rule, so it needs its own — otherwise a bridge carrying
  // no issues would still draw one.
  it("is hidden by its own rule, because it sets display", async () => {
    const css = readFileSync(resolve("src/styles/shell.css"), "utf8");
    expect(css).toMatch(/\.tb-issues \{[^}]*display:flex/);
    expect(css).toMatch(/\.tb-issues\[hidden\] \{[^}]*display:none/);
  });

  it("is the word and a bubble, with no icon", async () => {
    await standOnWorkspace();
    const entry = issues();
    expect(entry).not.toBeNull();
    expect(entry.textContent.trim()).toBe("Issues");
    expect(entry.querySelector("svg")).toBeNull();
    // The same count bubble the inbox rows wear.
    expect(entry.querySelector(".badge.tb-issues-count")).not.toBeNull();
  });

  it("sits with the tabs rather than with the cog", async () => {
    await standOnWorkspace();
    // Not in the right-hand cluster, which is the gear and the verb slot.
    expect(bar().querySelector(".tb-right [data-workspace-issues]")).toBeNull();
    // …and after the directory tabs, which it follows in the bar.
    const row = [...bar().querySelectorAll(".tb-directories, [data-workspace-issues]")];
    expect(row.map((node) => (node.dataset.workspaceIssues === "" ? "issues" : "directories")))
      .toEqual(["directories", "issues"]);
  });

  // It is deliberately NOT inside .tb-directories, which collapses into a menu
  // on a phone — the issues stay reachable at every width.
  it("stands outside the directory tabs, so the phone's collapse leaves it", async () => {
    await standOnWorkspace();
    expect(bar().querySelector(".tb-directories [data-workspace-issues]")).toBeNull();
  });

  it("is marked while the reader is on the tab, and on an issue opened from it", async () => {
    await standOnWorkspace();
    expect(issues().classList.contains("current")).toBe(false);

    App.route = { ...App.route, tab: "issues" };
    toolbarRouteChanged();
    await waitFor(() => expect(issues()?.classList.contains("current")).toBe(true));
    expect(issues().classList.contains("current")).toBe(true);
    expect(issues().getAttribute("aria-current")).toBe("page");

    App.route = { ...App.route, tab: "issues", issueId: "issue-1" };
    toolbarRouteChanged();
    await waitFor(() => expect(issues()?.classList.contains("current")).toBe(true));
    expect(issues().classList.contains("current")).toBe(true);
  });

  it("is the workspace's alone — no other identity carries one", async () => {
    for (const route of [{ name: "project", deviceId: "dev-1", projectId: "p1" }, { name: "inbox" }]) {
      App.route = route;
      toolbarRouteChanged();
      await waitFor(() => expect(bar()?.querySelector("[data-workspace-issues]")).toBeNull());
      expect([route.name, bar().querySelector("[data-workspace-issues]")]).toEqual([route.name, null]);
    }
  });
});

// Main's workspace toolbar, on the per-device model: the workspaces of the
// project the route is standing in are read from that project's own machine.
describe("the workspace toolbar", () => {
  it("keeps legacy deep-link identities readable without an active work menu", async () => {
    expect(names()).toEqual(["relaydb", "build/login"]);
    expect(bar().querySelector('[data-select="item"]')).toBeNull();
  });

  it("keeps the legacy project selector functional and opens it projects-first", async () => {
    const projects = await openJump("project");
    expect(projects.dataset.list).toBe("projects");
    expect(labels("[data-project]")).toEqual(["relaydb", "mascot"]);
  });

  it("shows only the workspace switcher before its directory tabs", async () => {
    await standOnWorkspace();
    expect(bar().querySelector('[data-select="project"]')).toBeNull();
    expect(bar().querySelector('[data-select="workspace"] .tb-name').textContent).toBe("payment-work");
    expect([...bar().children].indexOf(bar().querySelector('[data-select="workspace"]')))
      .toBeLessThan([...bar().children].indexOf(bar().querySelector(".tb-directories")));
  });

  it("lists the workspaces of the machine the route names, and asks no machine for them", async () => {
    await standOnWorkspace();
    await openJump("workspace");
    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
    expect(workshopCall).not.toHaveBeenCalledWith("workspace.list", expect.anything());
    expect(laptopCall).not.toHaveBeenCalledWith("workspace.list", expect.anything());
  });

  it("lists the current project's workspaces, then switches project in the same popup", async () => {
    await standOnWorkspace();
    const workspaces = await openJump("workspace");
    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
    expect(workspaces.querySelector("[data-projects]").textContent).toBe("Switch project");

    workspaces.querySelector("[data-projects]").click();
    await waitMenuList("projects");
    expect(labels("[data-project]")).toEqual(["relaydb", "mascot"]);
    menu().querySelector('[data-project="dev-1/p1"]').click();
    await waitMenuList("workspaces");
    await waitFor(() => expect(labels("[data-workspace]")).toEqual(["payment-work"]));

    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
    expect(menu().querySelector("[data-work]")).toBeNull();
    expect(menu().querySelector('[data-create="workspace"]')).toBeTruthy();
  });

  // The switcher and the rail row are two lists of the same workspaces, so a
  // checkout the bridge could not build says one thing in both places
  // (core/text.js `workspaceFailedText`).
  it("says a failed workspace failed, and why, in the words the rail row uses", async () => {
    await standOnWorkspace();
    const broken = {
      ...payments,
      id: "ws-2",
      workspace_id: "ws-2",
      name: "prototype",
      status: "failed",
      directories: [{ source_id: "web", status: "failed", error: "directory exists\nhint: reuse it" }],
    };
    workshopHolds({ p1: [payments, broken] });
    toolbarRouteChanged();
    await waitFor(() => expect(bar()?.querySelector('[data-select="workspace"]')).toBeTruthy());
    const row = (await openJump("workspace")).querySelector('[data-workspace="dev-1/ws-2"]');
    expect(row.querySelector(".md").textContent).toBe("Failed: directory exists");
  });

  it("goes to the workspace you pick", async () => {
    await standOnWorkspace();
    const sandbox = { ...payments, id: "ws-2", workspace_id: "ws-2", name: "prototype", directories: [] };
    workshopHolds({ p1: [payments, sandbox] });
    toolbarRouteChanged();
    await waitFor(() => expect(bar()?.querySelector('[data-select="workspace"]')).toBeTruthy());
    (await openJump("workspace")).querySelector('[data-workspace="dev-1/ws-2"]').click();
    await waitMenuClosed();
    expect(location.hash).toBe("#/device/dev-1/project/p1/workspace/ws-2/changes");
  });

  it("keeps the active workspace visible while browsing another project and resets scope when reopened", async () => {
    const sandbox = { id: "ws-2", project_id: "p2", name: "prototype", status: "ready", directories: [] };
    workshopHolds({ p1: [payments], p2: [sandbox] });
    await standOnWorkspace();

    (await openJump("workspace")).querySelector("[data-projects]").click();
    await waitMenuList("projects");
    menu().querySelector('[data-project="dev-1/p2"]').click();
    await waitMenuList("workspaces");
    expect(bar().querySelector('[data-select="workspace"] .tb-name').textContent).toBe("payment-work");
    expect([...bar().querySelectorAll("[data-directory]")].map((node) => node.textContent)).toEqual(["Frontend", "Design assets"]);
    await waitFor(() => expect(labels("[data-workspace]")).toEqual(["prototype"]));
    expect(labels("[data-workspace]")).toEqual(["prototype"]);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await waitMenuClosed();
    const reopened = await openJump("workspace");
    expect(reopened.querySelector(".tb-scope > span").textContent).toBe("relaydb");
    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
  });

  // The directories the tabs are drawn from ride the workspace list itself, so
  // reopening the menu on the project you are in draws them off the record
  // rather than reading the one row again.
  it("keeps the active workspace's directory tabs when its project is selected from the popup", async () => {
    await standOnWorkspace();

    (await openJump("workspace")).querySelector("[data-projects]").click();
    await waitMenuList("projects");
    menu().querySelector('[data-project="dev-1/p1"]').click();
    await waitMenuList("workspaces");
    await waitFor(() => expect(labels("[data-workspace]")).toEqual(["payment-work"]));

    expect(workshopCall).not.toHaveBeenCalledWith("workspace.get", expect.anything());
    expect([...bar().querySelectorAll("[data-directory]")].map((node) => node.textContent)).toEqual(["Frontend", "Design assets"]);
  });

  it("shows directory tabs and opens ordinary directories in Files", async () => {
    await standOnWorkspace();
    expect([...bar().querySelectorAll("[data-directory]")].map((node) => [node.textContent, node.getAttribute("aria-selected")])).toEqual([
      ["Frontend", "true"],
      ["Design assets", "false"],
    ]);
    bar().querySelector('[data-directory="assets"]').click();
    expect(location.hash).toBe("#/device/dev-1/project/p1/workspace/ws-1/directory/assets/files");
  });

  it("collapses directories into a phone menu without changing directory routing", async () => {
    await standOnWorkspace();
    const picker = bar().querySelector('[data-select="directory"]');
    expect(picker.textContent.trim()).toBe("Frontend▾");
    picker.click();
    await waitMenuList("directories");
    expect(picker.getAttribute("aria-expanded")).toBe("true");
    expect([...menu().querySelectorAll("[data-menu-directory]")].map((node) => [node.textContent.trim(), node.classList.contains("current")]))
      .toEqual([
        ["Frontend", true],
        ["Design assets", false],
      ]);
    expect(document.activeElement).toBe(menu().querySelector('[data-menu-directory="frontend"]'));
    expect(menu().querySelector('[data-menu-directory="frontend"]').getAttribute("aria-checked")).toBe("true");
    expect(menu().querySelector('[data-menu-directory="assets"]').getAttribute("aria-checked")).toBe("false");
    menu().querySelector('[data-menu-directory="assets"]').click();
    expect(location.hash).toBe("#/device/dev-1/project/p1/workspace/ws-1/directory/assets/files");
  });

  it("leaves finish and contextual actions out of the navigation toolbar", async () => {
    expect(bar().querySelector("#tb-verb").children).toHaveLength(0);
    expect(bar().querySelector('[data-select="more"]')).toBeNull();
  });
});

// The cog at the far right, opposite the switcher at the far left. It settles
// the workspace you are STANDING in, so a route that stands in none has none.
// The project's own checkout is the template every workspace is cut from, not
// a place to work. A machine running a bridge that still lists it (older ones
// did, as `legacy-<project>`) gets it kept out of the switcher here.
describe("the project's own checkout", () => {
  it("is never a row in the switcher, whatever the machine lists", async () => {
    const own = { id: "legacy-p1", workspace_id: "legacy-p1", project_id: "p1", name: "relaydb", root: "/repos/relaydb", directories: [] };
    workshopHolds({ p1: [own, payments] });
    await standOnWorkspace();
    await openJump("workspace");
    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
  });
});

// The chevron at the bar's left edge: the way back out of a workspace to the
// project it was cut from — the project's own page, the same place the
// project's name in the inbox opens.
describe("the back chevron", () => {
  const back = () => bar().querySelector("[data-project-back]");

  it("is absent on a legacy branch route, an issue and the inbox", async () => {
    expect(back()).toBeNull();
    App.route = { name: "issue", deviceId: "dev-1", projectId: "p1", id: "plan-1" };
    toolbarRouteChanged();
    expect(back()).toBeNull();
    App.route = { name: "inbox" };
    toolbarRouteChanged();
    expect(back()).toBeNull();
  });

  it("stands left of the workspace switcher, named for the project it goes back to", async () => {
    await standOnWorkspace();
    const button = back();
    expect(button).not.toBeNull();
    expect(button.getAttribute("aria-label")).toBe("Back to relaydb");
    expect(button.nextElementSibling).toBe(bar().querySelector('[data-select="workspace"]'));
  });

  it("takes you to the project, on the machine the workspace is on", async () => {
    await standOnWorkspace();
    back().click();
    // Leaving a workspace asks its view whether it may (App.routeLeaveGuard),
    // so the navigation settles a tick later.
    await waitFor(() => expect(location.hash).toBe("#/device/dev-1/project/p1"));
    expect(location.hash).toBe("#/device/dev-1/project/p1");
    // …which is the Issues tab, the project's default (#46). The inbox's
    // project name writes this same link (core/projectModel.js mints both), so
    // both ways back into a project land on the tracker.
    expect(routeFromHash(location.hash)).toMatchObject({ name: "project", projectId: "p1", tab: "issues" });
  });
});

describe("the workspace settings cog", () => {
  const cog = () => bar().querySelector("[data-workspace-settings]");

  it("is absent on a legacy branch route, an issue and the inbox", async () => {
    expect(cog()).toBeNull();
    App.route = { name: "issue", deviceId: "dev-1", projectId: "p1", id: "plan-1" };
    toolbarRouteChanged();
    expect(cog()).toBeNull();
    App.route = { name: "inbox" };
    toolbarRouteChanged();
    expect(cog()).toBeNull();
  });

  it("stands in the right-hand slot, before the verb, once the route is in a workspace", async () => {
    await standOnWorkspace();
    const button = cog();
    expect(button).not.toBeNull();
    expect(button.getAttribute("aria-label")).toBe("Workspace settings");
    expect(button.parentElement.classList.contains("tb-right")).toBe(true);
    expect(button.nextElementSibling.id).toBe("tb-verb");
  });

  it("opens the sheet on the workspace the bar is naming", async () => {
    await standOnWorkspace();
    cog().click();
    await waitFor(() => expect(document.getElementById("scrim").classList.contains("show")).toBe(true));
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
    expect(document.getElementById("wsname").value).toBe("payment-work");
    document.getElementById("wscancel").click();
  });
});

// The project menu, on main's chrome. Three things this bar used to carry are
// retired with main's workspace toolbar, so their cases are gone rather than
// restated: the item half's work menu on a legacy branch or issue route, the
// branch and issue creates (a workspace is the only thing the bar creates now),
// and the ⋯ menu.
describe("the project menu", () => {
  it("mounts its cached scope, open list and query without a click", async () => {
    await stopToolbar();
    App.route = { name: "inbox" };
    await writeCached(uiAddress({ view: "toolbar", kind: "filter", sub: "project" }), { projectKey: "dev-1/p2" });
    await writeCached(uiAddress({ view: "toolbar", kind: "menu", sub: "jump" }), {
      open: true, select: "project", list: "projects", query: "mas",
    });
    await initToolbar();
    expect(menu()?.dataset.list).toBe("projects");
    expect(menu().querySelector(".tb-filter").value).toBe("mas");
    expect(labels("[data-project]")).toEqual(["mascot"]);
    expect(menu().querySelector(".mi.current .mt").textContent).toBe("mascot");
  });

  it("repaints an open menu and project scope on external cache writes", async () => {
    const menuAddress = uiAddress({ view: "toolbar", kind: "menu", sub: "jump" });
    const scopeAddress = uiAddress({ view: "toolbar", kind: "filter", sub: "project" });
    await writeCached(menuAddress, { open: true, select: "project", list: "projects", query: "" });
    await waitMenuList("projects");
    await writeCached(scopeAddress, { projectKey: "dev-1/p2" });
    await waitFor(() => expect(menu().querySelector(".mi.current .mt").textContent).toBe("mascot"));
    await writeCached(menuAddress, { open: false });
    await waitMenuClosed();
  });

  it("lists projects and only projects, and shuts on Escape", async () => {
    const popup = await openJump("project");
    expect(labels("[data-project]")).toEqual(["relaydb", "mascot"]);
    expect(popup.querySelectorAll("[data-work]").length).toBe(0);
    expect(popup.querySelectorAll("[data-create]").length).toBe(0);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await waitMenuClosed();
  });

  it("filters the list against what you type", async () => {
    const filter = (await openJump("project")).querySelector(".tb-filter");
    filter.value = "masc";
    filter.dispatchEvent(new Event("input"));
    await waitFor(() => expect(labels("[data-project]")).toEqual(["mascot"]));
  });

  it("hands a picked project to its workspaces, without leaving the page", async () => {
    const popup = await openJump("project");
    popup.querySelector('[data-project="dev-1/p2"]').click();
    await waitMenuList("workspaces");
    await waitFor(() => expect(menu()?.querySelector(".tb-scope > span")?.textContent).toBe("mascot"));
    expect(menu()).toBeTruthy();
    expect(menu().querySelectorAll("[data-project]").length).toBe(0);
    expect(menu().querySelector(".tb-scope > span").textContent).toBe("mascot");
    expect(menu().textContent).toContain("No workspace by that name.");
    expect(location.hash).not.toContain("p2/branch");
  });

  it("still toggles shut after the feed repaints the button it hangs off", async () => {
    await openJump("project");
    await refreshFeed(); // a poll lands under the open menu
    expect(menu()).toBeTruthy();
    bar().querySelector('[data-select="project"]').click();
    await waitMenuClosed();
  });
});

// An open menu is reconciled row by row, not rewritten: a feed tick that says
// what the last one said touches nothing, and one that changes a single row
// touches only that row — so the box being typed into, and the caret in it,
// outlive every poll under the menu.
describe("the jump menu's paint", () => {
  /** Everything the DOM under `target` did while `act` ran. */
  const churn = async (target, act) => {
    const seen = [];
    const observer = new MutationObserver((records) => seen.push(...records));
    observer.observe(target, { childList: true, subtree: true, attributes: true, characterData: true });
    await act();
    seen.push(...observer.takeRecords());
    observer.disconnect();
    return seen;
  };

  const quiet = feed;
  afterAll(() => {
    feed = quiet;
  });

  it("touches nothing when the feed repeats what it already said", async () => {
    const popup = await openJump("project");
    expect(await churn(popup, () => refreshFeed())).toEqual([]);
  });

  it("redraws only the row that changed, and never the box being typed into", async () => {
    const popup = await openJump("project");
    const filter = popup.querySelector(".tb-filter");
    const rows = [...popup.querySelectorAll("[data-project]")];
    const records = await churn(popup, () => {
      feed = { ...quiet, items: [{ ...quiet.items[0], unread: true, unread_count: 3 }, quiet.items[1]] };
      return refreshFeed();
    });
    const relaydbRow = popup.querySelector('[data-project="dev-1/p1"]');
    expect(popup.querySelector(".tb-filter")).toBe(filter); // never replaced
    expect([...popup.querySelectorAll("[data-project]")]).toEqual(rows);
    expect(records.length).toBeGreaterThan(0);
    expect(records.every((record) => relaydbRow.contains(record.target))).toBe(true);
    feed = quiet;
    await refreshFeed();
  });
});

describe("the project you pick, against a feed that keeps ticking", () => {
  const quiet = feed;

  beforeEach(async () => {
    feed = {
      ...quiet,
      items: [
        ...quiet.items,
        {
          kind: "branch",
          deviceId: "dev-1",
          projectKey: "dev-1/p2",
          project_id: "p2",
          project: "mascot",
          branch: "build/spike",
          title: "Mascot spike",
          resume_at: ago(10),
        },
      ],
    };
    await refreshFeed();
  });

  afterAll(() => {
    feed = quiet;
  });

  it("holds the pick while you stand on another project's branch and the feed ticks", async () => {
    (await openJump("project")).querySelector('[data-project="dev-1/p2"]').click();
    await waitMenuList("workspaces");
    expect(menu().querySelector(".tb-scope > span").textContent).toBe("mascot");
    await refreshFeed(); // two seconds later…
    await refreshFeed(); // …and two more
    expect(menu().querySelector(".tb-scope > span").textContent).toBe("mascot");
  });

  it("still marks the picked project as the current one on the way back", async () => {
    (await openJump("project")).querySelector('[data-project="dev-1/p2"]').click();
    await waitMenuList("workspaces");
    await refreshFeed();
    menu().querySelector("[data-projects]").click();
    await waitMenuList("projects");
    expect(menu().querySelector(".mi.current .mt").textContent).toBe("mascot");
  });

  it("re-scopes to the project you navigate into", async () => {
    (await openJump("project")).querySelector('[data-project="dev-1/p2"]').click();
    await waitMenuList("workspaces");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await waitMenuClosed();
    App.route = { name: "issue", deviceId: "dev-1", projectId: "p1", id: "plan-1" };
    toolbarRouteChanged();
    await waitFor(async () => expect((await readCached(uiAddress({ view: "toolbar", kind: "filter", sub: "project" }))).value.projectKey).toBe("dev-1/p1"));
    expect((await openJump("project")).querySelector(".mi.current .mt").textContent).toBe("relaydb");
  });
});

describe("creating from the menu", () => {
  // A workspace is the only thing the bar creates, and it creates it on the
  // machine the scoped project is on — the bare id it is given means nothing
  // anywhere else.
  it("creates in the project the menu is scoped to, even with no workspace in it", async () => {
    workshopHolds({ p1: [] });
    await standOnWorkspace();
    (await openJump("workspace")).querySelector('[data-create="workspace"]').click();
    expect(openCreateWork).toHaveBeenCalledWith({
      projectId: "p1",
      deviceId: "dev-1",
      projectName: "relaydb",
      navigate: expect.any(Function),
    });
    await waitMenuClosed();
  });

  it("creates in the picked project on its own device", async () => {
    feed = {
      items: [],
      projects: [
        { id: "p1", deviceId: "dev-1", projectKey: "dev-1/p1", name: "relaydb", path: "/repos/relaydb" },
        { id: "p1", deviceId: "dev-2", projectKey: "dev-2/p1", name: "their relaydb" },
      ],
    };
    await refreshFeed();
    await standOnWorkspace();
    (await openJump("workspace")).querySelector("[data-projects]").click();
    await waitMenuList("projects");
    menu().querySelector('[data-project="dev-2/p1"]').click();
    await waitMenuList("workspaces");
    await waitFor(() => expect(menu()?.querySelector('[data-create="workspace"]')).toBeTruthy());
    menu().querySelector('[data-create="workspace"]').click();
    expect(openCreateWork).toHaveBeenCalledWith({
      projectId: "p1",
      deviceId: "dev-2",
      projectName: "their relaydb",
      navigate: expect.any(Function),
    });
    feed = savedFeed;
    await refreshFeed();
  });

  it("says so instead of opening anything when the account has no project", async () => {
    feed = { items: [], projects: [] };
    await refreshFeed();
    await standOnWorkspace();
    (await openJump("workspace")).querySelector('[data-create="workspace"]').click();
    expect(openCreateWork).not.toHaveBeenCalled();
    expect(notifyError).toHaveBeenCalledWith("No project to create in.", expect.any(String));
    feed = savedFeed;
    await refreshFeed();
  });
});

describe("the unread counters on the project menu", () => {
  const quiet = feed;
  const badges = (selector) => [...menu().querySelectorAll(selector)].map((row) => (row.querySelector(".badge") || {}).textContent || "");

  beforeEach(async () => {
    feed = {
      ...quiet,
      items: [
        { ...quiet.items[0], unread: true, unread_count: 2 },
        { ...quiet.items[1], unread: true, unread_count: 3 },
        {
          kind: "branch",
          deviceId: "dev-1",
          projectKey: "dev-1/p2",
          project_id: "p2",
          project: "mascot",
          branch: "build/spike",
          title: "",
          unread: true,
          unread_count: 4,
          resume_at: ago(10),
        },
      ],
    };
    await refreshFeed();
  });

  afterAll(() => {
    feed = quiet;
  });

  it("counts each project by the unread of the work inside it", async () => {
    await openJump("project");
    expect(badges("[data-project]")).toEqual(["5", "4"]);
  });

  it("wears no counter anywhere once everything has been read", async () => {
    feed = quiet;
    await refreshFeed();
    await openJump("project");
    expect(badges("[data-project]")).toEqual(["", ""]);
  });
});

describe("an account with more than one device", () => {
  it("offers every device's projects, naming the device on a clash", async () => {
    const mine = {
      items: [],
      projects: [
        { id: "p1", name: "relaydb", deviceId: "dev-1", projectKey: "dev-1/p1" },
        { id: "p2", name: "mascot", deviceId: "dev-1", projectKey: "dev-1/p2" },
      ],
    };
    const theirs = {
      items: [],
      projects: [{ id: "p1", name: "relaydb", deviceId: "dev-2", projectKey: "dev-2/p1" }],
    };
    feed = { items: [], projects: [...mine.projects, ...theirs.projects], devices: { "dev-1": mine, "dev-2": theirs } };
    await refreshFeed();
    App.route = { name: "inbox" };
    toolbarRouteChanged();
    const popup = await openJump("project");

    expect([...popup.querySelectorAll("[data-project]")].map((row) => row.dataset.project)).toEqual([
      "dev-1/p1",
      "dev-1/p2",
      "dev-2/p1",
    ]);
    expect(popup.querySelector('[data-project="dev-1/p1"] .mt').textContent).toBe("relaydb workshop");
    expect(popup.querySelector('[data-project="dev-2/p1"] .mt').textContent).toBe("relaydb laptop");
    expect(popup.querySelector('[data-project="dev-1/p2"] .mt .dim')).toBeNull();
    feed = savedFeed;
    await refreshFeed();
  });

  // The picker narrows the list of places to GO. Where you are standing is not
  // in that list: the bar goes on naming the project you are in, whichever
  // machine the filter is showing.
  it("the filter hides the other device's projects from the project menu and never changes the route", async () => {
    feed = {
      items: [...savedFeed.items],
      projects: [...savedFeed.projects, { id: "p1", name: "relaydb", deviceId: "dev-2", projectKey: "dev-2/p1" }],
    };
    await refreshFeed();
    const standing = App.route;

    await rememberDeviceFilter("dev-2");

    expect([...(await openJump("project")).querySelectorAll("[data-project]")].map((row) => row.dataset.project)).toEqual([
      "dev-2/p1",
    ]);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));

    // …and the branch the reader is standing on is still a branch of relaydb on
    // the workshop, named by the bar.
    expect(App.route).toBe(standing);
    expect(names()).toEqual(["relaydb", "build/login"]);

    await rememberDeviceFilter(null);
    feed = savedFeed;
    await refreshFeed();
  });

  // The scope outlives the page, and what it stores is the account-wide name of
  // the project — a bare id names one on every machine.
  it("the scope key round-trips a projectKey", async () => {
    App.route = { name: "inbox" };
    toolbarRouteChanged();
    (await openJump("project")).querySelector('[data-project="dev-1/p2"]').click();
    await waitMenuList("workspaces");

    const address = uiAddress({ view: "toolbar", kind: "filter", sub: "project" });
    const stored = (await readCached(address)).value.projectKey;
    expect(stored).toBe("dev-1/p2");
    expect(splitDeviceKey(stored)).toEqual({ deviceId: "dev-1", projectId: "p2" });

    // And the scope the key holds is the one the menu comes back to.
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await waitMenuClosed();
    toolbarRouteChanged();
    expect((await openJump("project")).querySelector(".mi.current .mt").textContent).toBe("mascot");
  });
});
