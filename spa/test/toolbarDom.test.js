// @vitest-environment jsdom
// The view-area toolbar on main's workspace chrome: the sentence it prints, the
// project menu behind its left half, the workspace switcher, and the directory
// tabs of the workspace the route is standing in — every one of them read from
// the machine the route names.

import { describe, it, expect, beforeEach, afterAll, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

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
    { id: "p1", deviceId: "dev-1", projectKey: "dev-1/p1", name: "relaydb" },
    { id: "p2", deviceId: "dev-1", projectKey: "dev-1/p2", name: "mascot" },
  ],
};
// A Set, matching the real module (core/taskFeed.js): this file's own
// navigation can land the router on a route it has already rendered (the
// hash unchanged), which calls render() straight through rather than via a
// hashchange listener this test never wires up — and that can mount the
// agent rail, which subscribes to the feed too. A single-slot stub would let
// that second subscriber silently steal the toolbar's own.
const subscribers = new Set();
const refreshFeed = vi.fn(async () => subscribers.forEach((fn) => fn(feed)));
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
const { initToolbar, stopToolbar, toolbarRouteChanged } = await import("../src/core/toolbar.js");
const { splitDeviceKey } = await import("../src/core/deviceKey.js");
const { adoptDeviceSession } = await import("../src/core/deviceContexts.js");
const { rememberDeviceFilter } = await import("../src/core/deviceFilter.js");

/** What each machine answers. A workspace read the bar makes is made on the
 *  machine the project it is about lives on, so the two are told apart. */
const workshopCall = vi.fn(async () => ({}));
const laptopCall = vi.fn(async () => ({}));
const openSession = (deviceId, call) => adoptDeviceSession({ deviceId, call, close: () => {}, peer: () => {}, onCarrier: () => {} });

const flush = () => new Promise((done) => setTimeout(done, 0));
const bar = () => document.querySelector("#toolbar .toolbar");
const menu = () => document.querySelector(".tbmenu");
const names = () => [...bar().querySelectorAll(".tb-name")].map((name) => name.textContent);
const openJump = (which = "project") => {
  bar().querySelector(`[data-select="${which}"]`).click();
  return menu();
};
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

/** The workshop answering workspace reads for whatever it has been given. */
const workshopHolds = (byProject) => {
  workshopCall.mockImplementation(async (method, params) => {
    if (method === "workspace.list") return { workspaces: byProject[params.project_id] || [] };
    if (method === "workspace.get") return { workspace: payments };
    return {};
  });
};

/** Stand on the workshop's payment-work, with its directory tabs hydrated. */
const standOnWorkspace = async () => {
  App.route = { name: "workspace", deviceId: "dev-1", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" };
  toolbarRouteChanged();
  await flush();
  await flush();
};

beforeEach(() => {
  if (!savedFeed) savedFeed = feed;
  stopToolbar();
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
  rememberDeviceFilter(null);
  workshopCall.mockReset();
  laptopCall.mockReset();
  workshopHolds({ p1: [payments] });
  laptopCall.mockImplementation(async () => ({}));
  openSession("dev-1", workshopCall);
  openSession("dev-2", laptopCall);
  initToolbar();
  toolbarRouteChanged();
});

afterAll(() => stopToolbar());

describe("the sentence the toolbar prints", () => {
  it("names the project, then the branch — the working time and diffstat pin above the agent rail's composer instead", () => {
    expect(names()).toEqual(["relaydb", "build/login"]);
    expect(document.getElementById("tb-status")).toBeNull();
  });

  it("names an issue by its title", () => {
    App.route = { name: "issue", deviceId: "dev-1", projectId: "p1", id: "plan-1" };
    toolbarRouteChanged();
    expect(names()).toEqual(["relaydb", "Add a health endpoint"]);
  });

  it("keeps the project selector on a route that is no work item", () => {
    App.route = { name: "inbox" };
    toolbarRouteChanged();
    expect(names()).toEqual(["relaydb"]);
    expect(bar().querySelector('[data-select="item"]')).toBeNull();
  });
});

// Main's workspace toolbar, on the per-device model: the workspaces of the
// project the route is standing in are read from that project's own machine.
describe("the workspace toolbar", () => {
  it("keeps legacy deep-link identities readable without an active work menu", () => {
    expect(names()).toEqual(["relaydb", "build/login"]);
    expect(bar().querySelector('[data-select="item"]')).toBeNull();
  });

  it("keeps the legacy project selector functional and opens it projects-first", () => {
    const projects = openJump("project");
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

  it("reads the workspaces from the machine the route names, not another device's", async () => {
    await standOnWorkspace();
    expect(workshopCall).toHaveBeenCalledWith("workspace.list", { project_id: "p1" });
    expect(laptopCall).not.toHaveBeenCalledWith("workspace.list", expect.anything());
  });

  it("lists the current project's workspaces, then switches project in the same popup", async () => {
    await standOnWorkspace();
    const workspaces = openJump("workspace");
    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
    expect(workspaces.querySelector("[data-projects]").textContent).toBe("Switch project");

    workspaces.querySelector("[data-projects]").click();
    expect(labels("[data-project]")).toEqual(["relaydb", "mascot"]);
    menu().querySelector('[data-project="dev-1/p1"]').click();
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
    const row = openJump("workspace").querySelector('[data-workspace="dev-1/ws-2"]');
    expect(row.querySelector(".md").textContent).toBe("Failed: directory exists");
  });

  it("goes to the workspace you pick", async () => {
    await standOnWorkspace();
    const sandbox = { ...payments, id: "ws-2", workspace_id: "ws-2", name: "prototype", directories: [] };
    workshopHolds({ p1: [payments, sandbox] });
    toolbarRouteChanged();
    await flush();
    openJump("workspace").querySelector('[data-workspace="dev-1/ws-2"]').click();
    expect(menu()).toBeNull();
    expect(location.hash).toBe("#/device/dev-1/project/p1/workspace/ws-2/changes");
  });

  it("keeps the active workspace visible while browsing another project and resets scope when reopened", async () => {
    const sandbox = { id: "ws-2", project_id: "p2", name: "prototype", status: "ready", directories: [] };
    workshopHolds({ p1: [payments], p2: [sandbox] });
    await standOnWorkspace();

    openJump("workspace").querySelector("[data-projects]").click();
    menu().querySelector('[data-project="dev-1/p2"]').click();
    expect(bar().querySelector('[data-select="workspace"] .tb-name').textContent).toBe("payment-work");
    expect([...bar().querySelectorAll("[data-directory]")].map((node) => node.textContent)).toEqual(["Frontend", "Design assets"]);
    await flush();
    expect(labels("[data-workspace]")).toEqual(["prototype"]);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    const reopened = openJump("workspace");
    expect(reopened.querySelector(".tb-scope > span").textContent).toBe("relaydb");
    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
  });

  it("rehydrates the active workspace when its project is selected from the popup", async () => {
    workshopHolds({ p1: [{ id: "ws-1", project_id: "p1", name: "payment-work", status: "ready" }] });
    await standOnWorkspace();

    openJump("workspace").querySelector("[data-projects]").click();
    menu().querySelector('[data-project="dev-1/p1"]').click();
    await flush();

    expect(workshopCall).toHaveBeenCalledWith("workspace.get", { workspace_id: "ws-1", thread_limit: 1 });
    expect([...bar().querySelectorAll("[data-directory]")].map((node) => node.textContent)).toEqual(["Frontend", "Design assets"]);
  });

  it("ignores a workspace list response overtaken by a newer project choice", async () => {
    let answerSandbox;
    const sandboxAnswer = new Promise((done) => {
      answerSandbox = done;
    });
    workshopCall.mockImplementation(async (method, params) => {
      if (method === "workspace.list" && params.project_id === "p2") return sandboxAnswer;
      if (method === "workspace.list") return { workspaces: [payments] };
      if (method === "workspace.get") return { workspace: payments };
      return {};
    });
    await standOnWorkspace();

    openJump("workspace").querySelector("[data-projects]").click();
    menu().querySelector('[data-project="dev-1/p2"]').click();
    menu().querySelector("[data-projects]").click();
    menu().querySelector('[data-project="dev-1/p1"]').click();
    await flush();
    answerSandbox({ workspaces: [{ id: "ws-2", project_id: "p2", name: "prototype" }] });
    await flush();

    expect(menu().querySelector(".tb-scope > span").textContent).toBe("relaydb");
    expect(labels("[data-workspace]")).toEqual(["payment-work"]);
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

  it("leaves finish and contextual actions out of the navigation toolbar", () => {
    expect(bar().querySelector("#tb-verb").children).toHaveLength(0);
    expect(bar().querySelector('[data-select="more"]')).toBeNull();
  });
});

// The cog at the far right, opposite the switcher at the far left. It settles
// the workspace you are STANDING in, so a route that stands in none has none.
describe("the workspace settings cog", () => {
  const cog = () => bar().querySelector("[data-workspace-settings]");

  it("is absent on a legacy branch route, an issue and the inbox", () => {
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
    await flush();
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
  it("lists projects and only projects, and shuts on Escape", () => {
    const popup = openJump("project");
    expect(labels("[data-project]")).toEqual(["relaydb", "mascot"]);
    expect(popup.querySelectorAll("[data-work]").length).toBe(0);
    expect(popup.querySelectorAll("[data-create]").length).toBe(0);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(menu()).toBeNull();
  });

  it("filters the list against what you type", () => {
    const filter = openJump("project").querySelector(".tb-filter");
    filter.value = "masc";
    filter.dispatchEvent(new Event("input"));
    expect(labels("[data-project]")).toEqual(["mascot"]);
  });

  it("hands a picked project to its workspaces, without leaving the page", async () => {
    const popup = openJump("project");
    popup.querySelector('[data-project="dev-1/p2"]').click();
    await flush();
    expect(menu()).toBeTruthy();
    expect(menu().querySelectorAll("[data-project]").length).toBe(0);
    expect(menu().querySelector(".tb-scope > span").textContent).toBe("mascot");
    expect(menu().textContent).toContain("No workspace by that name.");
    expect(location.hash).not.toContain("p2/branch");
  });

  it("still toggles shut after the feed repaints the button it hangs off", async () => {
    openJump("project");
    await refreshFeed(); // a poll lands under the open menu
    expect(menu()).toBeTruthy();
    bar().querySelector('[data-select="project"]').click();
    expect(menu()).toBeNull();
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
    const popup = openJump("project");
    expect(await churn(popup, () => refreshFeed())).toEqual([]);
  });

  it("redraws only the row that changed, and never the box being typed into", async () => {
    const popup = openJump("project");
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
    openJump("project").querySelector('[data-project="dev-1/p2"]').click();
    expect(menu().querySelector(".tb-scope > span").textContent).toBe("mascot");
    await refreshFeed(); // two seconds later…
    await refreshFeed(); // …and two more
    expect(menu().querySelector(".tb-scope > span").textContent).toBe("mascot");
  });

  it("still marks the picked project as the current one on the way back", async () => {
    openJump("project").querySelector('[data-project="dev-1/p2"]').click();
    await refreshFeed();
    menu().querySelector("[data-projects]").click();
    expect(menu().querySelector(".mi.current .mt").textContent).toBe("mascot");
  });

  it("re-scopes to the project you navigate into", () => {
    openJump("project").querySelector('[data-project="dev-1/p2"]').click();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    App.route = { name: "issue", deviceId: "dev-1", projectId: "p1", id: "plan-1" };
    toolbarRouteChanged();
    expect(openJump("project").querySelector(".mi.current .mt").textContent).toBe("relaydb");
  });
});

describe("creating from the menu", () => {
  // A workspace is the only thing the bar creates, and it creates it on the
  // machine the scoped project is on — the bare id it is given means nothing
  // anywhere else.
  it("creates in the project the menu is scoped to, even with no workspace in it", async () => {
    workshopHolds({ p1: [] });
    await standOnWorkspace();
    openJump("workspace").querySelector('[data-create="workspace"]').click();
    expect(openCreateWork).toHaveBeenCalledWith({
      projectId: "p1",
      deviceId: "dev-1",
      projectName: "relaydb",
      navigate: expect.any(Function),
    });
    expect(menu()).toBeNull();
  });

  it("creates in the picked project on its own device", async () => {
    feed = {
      items: [],
      projects: [
        { id: "p1", deviceId: "dev-1", projectKey: "dev-1/p1", name: "relaydb" },
        { id: "p1", deviceId: "dev-2", projectKey: "dev-2/p1", name: "their relaydb" },
      ],
    };
    await refreshFeed();
    await standOnWorkspace();
    openJump("workspace").querySelector("[data-projects]").click();
    menu().querySelector('[data-project="dev-2/p1"]').click();
    await flush();
    menu().querySelector('[data-create="workspace"]').click();
    expect(laptopCall).toHaveBeenCalledWith("workspace.list", { project_id: "p1" });
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
    openJump("workspace").querySelector('[data-create="workspace"]').click();
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

  it("counts each project by the unread of the work inside it", () => {
    openJump("project");
    expect(badges("[data-project]")).toEqual(["5", "4"]);
  });

  it("wears no counter anywhere once everything has been read", async () => {
    feed = quiet;
    await refreshFeed();
    openJump("project");
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
    const popup = openJump("project");

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

    rememberDeviceFilter("dev-2");

    expect([...openJump("project").querySelectorAll("[data-project]")].map((row) => row.dataset.project)).toEqual([
      "dev-2/p1",
    ]);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));

    // …and the branch the reader is standing on is still a branch of relaydb on
    // the workshop, named by the bar.
    expect(App.route).toBe(standing);
    expect(names()).toEqual(["relaydb", "build/login"]);

    rememberDeviceFilter(null);
    feed = savedFeed;
    await refreshFeed();
  });

  // The scope outlives the page, and what it stores is the account-wide name of
  // the project — a bare id names one on every machine.
  it("the scope key round-trips a projectKey", () => {
    App.route = { name: "inbox" };
    toolbarRouteChanged();
    openJump("project").querySelector('[data-project="dev-1/p2"]').click();

    expect(localStorage.getItem("build.toolbar.project")).toBe("dev-1/p2");
    expect(splitDeviceKey(localStorage.getItem("build.toolbar.project"))).toEqual({ deviceId: "dev-1", projectId: "p2" });

    // And the scope the key holds is the one the menu comes back to.
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    toolbarRouteChanged();
    expect(openJump("project").querySelector(".mi.current .mt").textContent).toBe("mascot");
  });
});
