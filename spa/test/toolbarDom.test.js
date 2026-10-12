// @vitest-environment jsdom
// The view-area toolbar: real route ancestry, independent global and sibling
// controls, and cached destination rows from the machine the route names.

import { describe, it, expect, beforeEach, afterAll, vi } from "vitest";
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
      kind: "task",
      deviceId: "dev-1",
      projectKey: "dev-1/p1",
      project_id: "p1",
      project: "relaydb",
      branch: null,
      task_id: "plan-1",
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
const { clearToolbarVerb, initToolbar, setToolbarVerb, stopToolbar, toolbarRouteChanged } = await import("../src/core/toolbar.js");
const { splitDeviceKey } = await import("../src/core/deviceKey.js");
const { routeFromHash } = await import("../src/core/router.js");
const { adoptDeviceSession } = await import("../src/core/deviceContexts.js");
const { rememberDeviceFilter } = await import("../src/core/deviceFilter.js");
const { stampWorkspace } = await import("../src/core/feedMerge.js");
const { writeCached, wipeCache } = await import("../src/core/localCache.js");
const { readUiRecord, writeUiRecord, wipeUiRecords } = await import("../src/core/localUiStore.js");
const { uiAddress } = await import("../src/core/localUiState.js");
const { rememberProjectRailRoute } = await import("../src/core/projectRailState.js");
const { taskAddress } = await import("../src/core/trackerCache.js");

/** What each machine answers. A workspace read the bar makes is made on the
 *  machine the project it is about lives on, so the two are told apart. */
const workshopCall = vi.fn(async () => ({}));
const laptopCall = vi.fn(async () => ({}));
const openSession = (deviceId, call) => adoptDeviceSession({ deviceId, call, close: () => {}, peer: () => {}, onCarrier: () => {} });

const flush = () => new Promise((done) => setTimeout(done, 0));
const bar = () => document.querySelector("#toolbar .toolbar");
const menu = () => document.querySelector(".tbmenu");
const names = () => [...bar().querySelectorAll(".tb-name")]
  .filter((name) => App.route.name === "workspace" || !name.closest('[data-select="workspace"]'))
  .map((name) => name.textContent);
const openJump = async (which = "project") => {
  bar().querySelector(`[data-select="${which}"]`).click();
  // Keep this before the first await: a touch gesture must focus search in
  // its activating turn, before UI-store work can consume the gesture.
  expect(document.activeElement).toBe(menu()?.querySelector(".tb-filter"));
  const list = { project: "projects", workspace: "workspaces" }[which];
  await vi.waitFor(() => expect(menu()?.dataset.list).toBe(list));
  return menu();
};
const waitMenuList = (list) => vi.waitFor(() => expect(menu()?.dataset.list).toBe(list));
const waitMenuClosed = () => vi.waitFor(() => expect(menu()).toBeNull());
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

/** Stand on the workshop's payment-work, with its record hydrated. */
const standOnWorkspace = async () => {
  App.route = { name: "workspace", deviceId: "dev-1", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" };
  toolbarRouteChanged();
  await flush();
  await flush();
};

beforeEach(async () => {
  if (!savedFeed) savedFeed = feed;
  App.viewDispose?.();
  App.viewDispose = null;
  App.routeLeaveGuard = null;
  await stopToolbar();
  await wipeCache();
  await wipeUiRecords();
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

  it("names a task by its title", async () => {
    App.route = { name: "task", deviceId: "dev-1", projectId: "p1", id: "plan-1" };
    toolbarRouteChanged();
    expect(names()).toEqual(["relaydb", "Add a health endpoint"]);
  });

  it("keeps the global selector without inventing a project ancestor on an account route", async () => {
    App.route = { name: "inbox" };
    toolbarRouteChanged();
    expect(names()).toEqual([]);
    expect(bar().querySelector(".tb-project")).toBeNull();
    expect(bar().querySelector('[data-select="project"]')).not.toBeNull();
    expect(bar().querySelector('[data-select="item"]')).toBeNull();
  });
});

// Project faces stay in the rail; the toolbar exposes the project ancestor
// and a sibling-workspace shortcut.
describe("the project's pages", () => {
  const standOn = (route) => {
    App.route = route;
    toolbarRouteChanged();
  };

  it("keeps face tabs in the rail while offering a local workspace shortcut", async () => {
    standOn({ name: "project", deviceId: "dev-1", projectId: "p1" });
    expect(names()).toEqual(["relaydb"]);
    expect(bar().querySelector("[data-project-tab], [role='tablist']")).toBeNull();
    expect(bar().querySelector('[data-select="workspace"]').textContent).toContain("Workspaces");
    standOn({ name: "trackerTask", deviceId: "dev-1", projectId: "p1", taskId: "task-1" });
    expect(bar().querySelector("[data-project-tab], [role='tablist']")).toBeNull();
  });
});

describe("task ancestry from the URL", () => {
  const taskParent = () => [...bar().querySelectorAll("a")].find((link) => link.textContent === "Tasks");

  it("keeps a project-hosted task under project Tasks despite a workspace assignment", async () => {
    App.route = { name: "trackerTask", deviceId: "dev-1", projectId: "p1", taskId: "hosted-task" };
    toolbarRouteChanged();
    expect(taskParent()).toBeDefined();
    expect(routeFromHash(taskParent().getAttribute("href"))).toMatchObject({ name: "project", deviceId: "dev-1", projectId: "p1", tab: "tasks" });
    expect(bar().textContent).toContain("hosted-task");
    await writeCached(taskAddress("dev-1", "p1", "hosted-task"), {
      task: { id: "hosted-task", number: 542, workspace_id: "ws-1", title: "Navigation" }, timeline: [],
    });
    await vi.waitFor(() => expect(bar().textContent).toContain("#542"));
    expect(routeFromHash(taskParent().getAttribute("href"))).toMatchObject({ name: "project", deviceId: "dev-1", projectId: "p1", tab: "tasks" });
    expect(bar().querySelector('[data-select="workspace"] .tb-name').textContent).toBe("Workspaces");
    expect(workshopCall).not.toHaveBeenCalled();
  });

  it("keeps a workspace-hosted task under that workspace's Tasks layout despite reassignment", async () => {
    await standOnWorkspace();
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p1", workspaceId: "ws-1", tab: "tasks", taskId: "hosted-task", view: "board" };
    toolbarRouteChanged();
    expect(taskParent()).toBeDefined();
    const parent = routeFromHash(taskParent().getAttribute("href"));
    expect(parent).toMatchObject({ name: "workspace", deviceId: "dev-1", projectId: "p1", workspaceId: "ws-1", tab: "tasks", view: "board" });
    expect(parent.taskId).toBeUndefined();
    await writeCached(taskAddress("dev-1", "p1", "hosted-task"), {
      task: { id: "hosted-task", number: 543, workspace_id: "ws-other", title: "Moved assignment" }, timeline: [],
    });
    await vi.waitFor(() => expect(bar().textContent).toContain("#543"));
    expect(routeFromHash(taskParent().getAttribute("href"))).toEqual(parent);
    expect(bar().querySelector('[data-select="workspace"] .tb-name').textContent).toBe("payment-work");
    expect(workshopCall).not.toHaveBeenCalled();
  });
});

// The project is an ancestor link, while the workspace remains a separate
// selector. Face navigation stays in the workspace's own rail.
describe("the workspace breadcrumbs", () => {
  const picker = () => bar().querySelector('[data-select="workspace"]');

  it("keeps the up-arrow, project link and workspace selector independent", async () => {
    await standOnWorkspace();
    const project = bar().querySelector(".tb-project");
    const global = bar().querySelector('[data-select="project"]');
    expect(global.getAttribute("aria-label")).toBe("Switch project or workspace");
    expect(global.textContent).not.toContain("Switch");
    expect(project.tagName).toBe("A");
    expect(project.textContent).toBe("relaydb");
    expect(project.closest("nav").getAttribute("aria-label")).toBeTruthy();
    expect(project.closest("ol")).not.toBeNull();
    expect(picker().contains(project)).toBe(false);
    expect(picker().querySelector(".tb-name").textContent).toBe("payment-work");
    expect(picker().textContent.replace(/\s+/g, " ").trim()).toBe("payment-work▾");
    expect([...bar().querySelectorAll("button, a")]).toEqual([global, project, picker()]);
    expect(project.getAttribute("aria-label")).toBe("Open project workspace relaydb");
    expect(picker().getAttribute("aria-label")).toBe("Switch workspace in relaydb; current: payment-work");
  });

  it("leaves face navigation and contextual verbs on the workspace surface", async () => {
    await standOnWorkspace();
    expect([...bar().querySelectorAll("button")]).toEqual([bar().querySelector('[data-select="project"]'), picker()]);
    for (const gone of [".tb-directories", "[data-directory]", "[data-workspace-tasks]", "[data-workspace-settings]", ".tb-back", "[data-project-back]"]) {
      expect([gone, bar().querySelector(gone)]).toEqual([gone, null]);
    }
    // No verb slot either: a failed workspace's Retry is on its surface.
    expect(bar().querySelector("#tb-verb")).toBeNull();
    expect(bar().querySelector(".tb-right")).toBeNull();
  });

  // The local picker also exposes the saved project return destination.
  it("reaches the project's own page from its menu", async () => {
    await standOnWorkspace();
    const workspaces = await openJump("workspace");
    const page = workspaces.querySelector("[data-project-page]");
    expect(page.querySelector(".mt").textContent).toBe("Project workspace");
    page.click();
    await waitMenuClosed();
    // Project return resolves remembered local UI state before navigating.
    await vi.waitFor(() => expect(location.hash).toBe("#/device/dev-1/project/p1"));
    // …which is the Tasks tab, the project's default (#46). The inbox's
    // project name writes this same link (core/projectModel.js mints both).
    expect(routeFromHash(location.hash)).toMatchObject({ name: "project", projectId: "p1", tab: "tasks" });
  });

  it("returns from a workspace to the remembered project Files face", async () => {
    await rememberProjectRailRoute({ name: "project", deviceId: "dev-1", projectId: "p1", tab: "files", sourceId: "docs", file: "README.md" });
    await standOnWorkspace();
    (await openJump("workspace")).querySelector("[data-project-page]").click();
    await waitMenuClosed();
    await vi.waitFor(() => expect(routeFromHash(location.hash)).toMatchObject({
      name: "project", deviceId: "dev-1", projectId: "p1", tab: "files", sourceId: "docs", file: "README.md",
    }));
  });

  it("advertises the saved project face on the return row before a modified click", async () => {
    const saved = { name: "project", deviceId: "dev-1", projectId: "p1", tab: "files", sourceId: "docs", file: "README.md", line: 12 };
    await rememberProjectRailRoute(saved);
    await standOnWorkspace();
    const row = (await openJump("workspace")).querySelector("[data-project-page]");
    await vi.waitFor(() => expect(routeFromHash(row.getAttribute("href"))).toMatchObject(saved));
    const standing = App.route;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, ctrlKey: true }));
    expect(menu()).not.toBeNull();
    expect(App.route).toBe(standing);
  });

  it("returns through the project crumb in one activation without opening a picker", async () => {
    await rememberProjectRailRoute({ name: "project", deviceId: "dev-1", projectId: "p1", tab: "files", sourceId: "docs", file: "README.md" });
    await standOnWorkspace();
    const project = bar().querySelector(".tb-project");
    expect(project).not.toBeNull();
    await vi.waitFor(() => expect(routeFromHash(project.getAttribute("href"))).toMatchObject({
      name: "project", deviceId: "dev-1", projectId: "p1", tab: "files", sourceId: "docs", file: "README.md",
    }));
    project.click();
    expect(menu()).toBeNull();
    await vi.waitFor(() => expect(routeFromHash(location.hash)).toMatchObject({
      name: "project", deviceId: "dev-1", projectId: "p1", tab: "files", sourceId: "docs", file: "README.md",
    }));
  });

  it.each(["project", "workspace"])("focuses search synchronously and returns Escape to the exact %s trigger", async (which) => {
    await standOnWorkspace();
    const trigger = bar().querySelector(`[data-select="${which}"]`);
    trigger.focus();
    trigger.click();
    expect(document.activeElement).toBe(menu()?.querySelector(".tb-filter"));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await waitMenuClosed();
    expect(document.activeElement).toBe(trigger);
    expect(trigger.isConnected).toBe(true);
  });

  it("keeps the global switcher available on legacy and project routes", async () => {
    // The branch itself remains one leaf, even when its name contains slashes.
    expect(bar().querySelector('[data-select="project"]')).not.toBeNull();
    expect(bar().querySelector(".tb-crumb")).toBeNull();
    App.route = { name: "project", deviceId: "dev-1", projectId: "p1" };
    toolbarRouteChanged();
    await flush();
    expect(bar().querySelector('[data-select="project"]')).not.toBeNull();
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
    expect(labels("[data-project]")).toEqual(["mascot", "relaydb"]);
  });

  it("lists the workspaces of the machine the route names, and asks no machine for them", async () => {
    await standOnWorkspace();
    await openJump("workspace");
    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
    expect(workshopCall).not.toHaveBeenCalledWith("workspace.list", expect.anything());
    expect(laptopCall).not.toHaveBeenCalledWith("workspace.list", expect.anything());
  });

  it("keeps the current child first, then live siblings alphabetical irrespective of status", async () => {
    const child = (id, name, status) => ({ ...payments, id, workspace_id: id, name, status });
    workshopHolds({ p1: [
      child("ws-z", "Zebra", "ready"), payments,
      child("ws-a", "Alpha", "failed"), child("ws-b", "Beta", "provisioning"),
      child("ws-f", "Archived", "finished"),
    ] });
    await standOnWorkspace();
    await openJump("workspace");
    expect(labels("[data-workspace]")).toEqual(["payment-work", "Alpha", "Beta", "Zebra"]);
    const filter = menu().querySelector(".tb-filter");
    filter.value = "Alpha";
    filter.dispatchEvent(new Event("input"));
    await vi.waitFor(() => expect(labels("[data-workspace]")).toEqual(["Alpha"]));
    expect(workshopCall).not.toHaveBeenCalled();
    expect(laptopCall).not.toHaveBeenCalled();
  });

  it("lists the current project's workspaces, then switches project in the same popup", async () => {
    await standOnWorkspace();
    const workspaces = await openJump("workspace");
    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
    expect(workspaces.querySelector("[data-projects]").textContent).toBe("All projects");

    workspaces.querySelector("[data-projects]").click();
    await waitMenuList("projects");
    expect(labels("[data-project]")).toEqual(["mascot", "relaydb"]);
    menu().querySelector('[data-project="dev-1/p1"]').click();
    await waitMenuList("workspaces");
    await flush();

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
    await flush();
    const row = (await openJump("workspace")).querySelector('[data-workspace="dev-1/ws-2"]');
    expect(row.querySelector(".md").textContent).toBe("Failed: directory exists");
  });

  it("goes to the workspace you pick", async () => {
    await standOnWorkspace();
    const sandbox = { ...payments, id: "ws-2", workspace_id: "ws-2", name: "prototype", directories: [] };
    workshopHolds({ p1: [payments, sandbox] });
    toolbarRouteChanged();
    await flush();
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
    await flush();
    expect(labels("[data-workspace]")).toEqual(["prototype"]);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await waitMenuClosed();
    const reopened = await openJump("workspace");
    expect(reopened.querySelector(".tb-scope > span").textContent).toBe("relaydb");
    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
  });

  it("does not let a dismissed global project pick steal a newly opened local scope", async () => {
    const foreign = { id: "ws-other", project_id: "p2", name: "mascot checkout", status: "ready", directories: [] };
    workshopHolds({ p1: [payments], p2: [foreign] });
    await standOnWorkspace();
    (await openJump("project")).querySelector('[data-project="dev-1/p2"]').click();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    const local = await openJump("workspace");
    // Let the dismissed project's IndexedDB write and readback finish; only
    // the still-open local interaction is allowed to own this menu's scope.
    await new Promise((done) => setTimeout(done, 50));
    expect(menu()).toBe(local);
    expect(local.querySelector(".tb-scope > span").textContent).toBe("relaydb");
    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
    expect(document.activeElement).toBe(local.querySelector(".tb-filter"));
  });

  it("closes once when a rebuilt selector receives pointerdown then click", async () => {
    await standOnWorkspace();
    await openJump("workspace");
    const original = bar().querySelector('[data-select="workspace"]');
    const projects = feed.projects;
    try {
      feed = { ...feed, projects: projects.map((project) => project.projectKey === "dev-1/p1" ? { ...project, name: "Renamed relaydb" } : project) };
      await refreshFeed();
      const rebuilt = bar().querySelector('[data-select="workspace"]');
      expect(rebuilt).not.toBe(original);
      expect(original.isConnected).toBe(false);
      rebuilt.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
      rebuilt.click();
      await waitMenuClosed();
    } finally {
      feed = { ...feed, projects };
      await refreshFeed();
    }
  });

  // The workspace rides the workspace list itself, so reopening the menu on the
  // project you are in draws it off the record rather than reading the one row
  // again.
  it("keeps naming the active workspace when its project is selected from the popup", async () => {
    await standOnWorkspace();

    (await openJump("workspace")).querySelector("[data-projects]").click();
    await waitMenuList("projects");
    menu().querySelector('[data-project="dev-1/p1"]').click();
    await waitMenuList("workspaces");
    await flush();

    expect(workshopCall).not.toHaveBeenCalledWith("workspace.get", expect.anything());
    expect(bar().querySelector('[data-select="workspace"] .tb-name').textContent).toBe("payment-work");
  });

  it("leaves finish and contextual actions out of the navigation toolbar", async () => {
    expect(bar().querySelector("#tb-verb").children).toHaveLength(0);
    expect(bar().querySelector('[data-select="more"]')).toBeNull();
  });

  // Only a workspace's bar lost its verb slot (#174): a branch still stands its
  // Done there (views/branchView.js).
  it("keeps the verb slot on a branch, where its view writes Done", async () => {
    const done = (host) => { host.innerHTML = '<button class="btn mini" type="button">Done</button>'; };
    setToolbarVerb(done);
    expect(bar().querySelector("#tb-verb .btn").textContent).toBe("Done");
    clearToolbarVerb(done);
    expect(bar().querySelector("#tb-verb").children).toHaveLength(0);
  });
});

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

// The project menu, on main's chrome. Three things this bar used to carry are
// retired with main's workspace toolbar, so their cases are gone rather than
// restated: the item half's work menu on a legacy branch or task route, the
// branch and task creates (a workspace is the only thing the bar creates now),
// and the ⋯ menu.
describe("the project menu", () => {
  it("mounts its cached scope, open list and query without a click", async () => {
    await stopToolbar();
    App.route = { name: "inbox" };
    await writeUiRecord(uiAddress({ view: "toolbar", kind: "filter", sub: "project" }), { projectKey: "dev-1/p2" });
    await writeUiRecord(uiAddress({ view: "toolbar", kind: "menu", sub: "jump" }), {
      open: true, select: "project", list: "projects", query: "mas",
    });
    await initToolbar();
    expect(menu()?.dataset.list).toBe("projects");
    expect(menu().querySelector(".tb-filter").value).toBe("mas");
    expect(labels("[data-project]")).toEqual(["mascot"]);
    expect(menu().querySelector(".mi.current .mt").textContent).toBe("mascot");
  });

  it("applies external menu writes while preserving the active popup scope", async () => {
    const menuAddress = uiAddress({ view: "toolbar", kind: "menu", sub: "jump" });
    const scopeAddress = uiAddress({ view: "toolbar", kind: "filter", sub: "project" });
    await writeUiRecord(menuAddress, { open: true, select: "project", list: "projects", query: "" });
    await waitMenuList("projects");
    await writeUiRecord(scopeAddress, { projectKey: "dev-1/p2" });
    expect((await readUiRecord(scopeAddress)).value.projectKey).toBe("dev-1/p2");
    expect(menu().querySelector(".mi.current .mt").textContent).toBe("relaydb");
    await writeUiRecord(menuAddress, { open: false });
    await waitMenuClosed();
    await stopToolbar();
    App.route = { name: "inbox" };
    await initToolbar();
    expect((await openJump("project")).querySelector(".mi.current .mt").textContent).toBe("mascot");
  });

  it("lists projects and only projects, and shuts on Escape", async () => {
    const popup = await openJump("project");
    expect(labels("[data-project]")).toEqual(["mascot", "relaydb"]);
    expect(popup.querySelectorAll("[data-work]").length).toBe(0);
    expect(popup.querySelectorAll("[data-create]").length).toBe(0);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await waitMenuClosed();
  });

  it("filters the list against what you type", async () => {
    const filter = (await openJump("project")).querySelector(".tb-filter");
    filter.value = "masc";
    filter.dispatchEvent(new Event("input"));
    await vi.waitFor(() => expect(labels("[data-project]")).toEqual(["mascot"]));
  });

  it("hands a picked project to its workspaces, without leaving the page", async () => {
    const popup = await openJump("project");
    popup.querySelector('[data-project="dev-1/p2"]').click();
    await waitMenuList("workspaces");
    await flush();
    expect(menu()).toBeTruthy();
    expect(menu().querySelectorAll("[data-project]").length).toBe(0);
    expect(menu().querySelector(".tb-scope > span").textContent).toBe("mascot");
    expect(menu().textContent).toContain("No cached workspaces yet");
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
    App.route = { name: "task", deviceId: "dev-1", projectId: "p1", id: "plan-1" };
    toolbarRouteChanged();
    await vi.waitFor(async () => expect((await readUiRecord(uiAddress({ view: "toolbar", kind: "filter", sub: "project" }))).value.projectKey).toBe("dev-1/p1"));
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
    await flush();
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
    expect(badges("[data-project]")).toEqual(["4", "5"]);
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
      "dev-1/p2",
      "dev-2/p1",
      "dev-1/p1",
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
    expect(names()).toEqual(["relaydb · workshop", "build/login"]);

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
    const stored = (await readUiRecord(address)).value.projectKey;
    expect(stored).toBe("dev-1/p2");
    expect(splitDeviceKey(stored)).toEqual({ deviceId: "dev-1", projectId: "p2" });

    // And the scope the key holds is the one the menu comes back to.
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await waitMenuClosed();
    toolbarRouteChanged();
    expect((await openJump("project")).querySelector(".mi.current .mt").textContent).toBe("mascot");
  });
});
