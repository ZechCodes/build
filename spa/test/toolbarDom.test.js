// @vitest-environment jsdom
// The view-area toolbar's wiring: the sentence it prints, the menu each half
// opens, the two creates behind the work half, and the ⋯ on its right.

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
    { id: "p1", name: "relaydb" },
    { id: "p2", name: "mascot" },
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
  primaryRunIdFor: () => null,
}));
const openProjectSettings = vi.fn();
vi.mock("../src/sheets/projectSettings.js", () => ({ openProjectSettings: (...args) => openProjectSettings(...args) }));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notify: () => {} }));
const openCreateWork = vi.fn();
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: (...args) => openCreateWork(...args) }));

const { App } = await import("../src/app.js");
const { initToolbar, stopToolbar, toolbarRouteChanged } = await import("../src/core/toolbar.js");

const flush = () => new Promise((done) => setTimeout(done, 0));
const bar = () => document.querySelector("#toolbar .toolbar");
const menu = () => document.querySelector(".tbmenu");
const names = () => [...bar().querySelectorAll(".tb-name")].map((name) => name.textContent);
const openJump = (which = "project") => {
  bar().querySelector(`[data-select="${which}"]`).click();
  return menu();
};

beforeEach(() => {
  if (!savedFeed) savedFeed = feed;
  if (!document.getElementById("shell")) document.body.innerHTML = bodyHtml;
  localStorage.clear();
  App.gated = false;
  App.route = { name: "branch", projectId: "p1", branch: "build/login", tab: "changes" };
  App.focusComposerOnMount = false;
  openCreateWork.mockClear();
  notifyError.mockClear();
  App.call = vi.fn(async (method) => {
    if (method === "worktree.create") return { project_id: "p1", branch: "build/mascot-model-spike", worktree_id: "wt-9" };
    if (method === "issue.create") return { project_id: "p1", issue_id: "plan-9", plan_id: "plan-9" };
    return {};
  });
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
    App.route = { name: "issue", projectId: "p1", id: "plan-1" };
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

describe("the two menus, one per half", () => {
  it("lists projects and only projects on the project half", () => {
    const popup = openJump("project");
    expect([...popup.querySelectorAll("[data-project]")].map((row) => row.textContent.trim())).toEqual(["relaydb", "mascot"]);
    expect(popup.querySelectorAll("[data-work]").length).toBe(0);
    expect(popup.querySelectorAll("[data-create]").length).toBe(0);
  });

  it("lists the scoped project's work and only its work on the item half, with both creates", () => {
    const popup = openJump("item");
    expect([...popup.querySelectorAll("[data-work]")].map((row) => row.querySelector(".mt").textContent)).toEqual([
      "Add a health endpoint",
      "build/login",
    ]);
    expect(popup.querySelectorAll("[data-project]").length).toBe(0);
    expect([...popup.querySelectorAll("[data-create]")].map((row) => row.dataset.create)).toEqual(["branch", "issue"]);
  });

  it("filters each menu against its own list", () => {
    const projectFilter = openJump("project").querySelector(".tb-filter");
    projectFilter.value = "masc";
    projectFilter.dispatchEvent(new Event("input"));
    expect([...menu().querySelectorAll("[data-project]")].map((row) => row.textContent.trim())).toEqual(["mascot"]);

    const workFilter = openJump("item").querySelector(".tb-filter");
    workFilter.value = "login";
    workFilter.dispatchEvent(new Event("input"));
    expect([...menu().querySelectorAll("[data-work]")].map((row) => row.querySelector(".mt").textContent)).toEqual(["build/login"]);
  });

  it("hands a picked project to its work list, without leaving the page", () => {
    const popup = openJump("project");
    popup.querySelector('[data-project="p2"]').click();
    expect(menu()).toBeTruthy();
    expect(menu().querySelectorAll("[data-project]").length).toBe(0);
    expect(menu().querySelectorAll("[data-work]").length).toBe(0);
    expect(menu().textContent).toContain("Nothing here yet.");
    expect(location.hash).not.toContain("p2/branch");
  });

  it("goes back to the projects from the work list", () => {
    openJump("item").querySelector("[data-projects]").click();
    expect([...menu().querySelectorAll("[data-project]")].map((row) => row.textContent.trim())).toEqual(["relaydb", "mascot"]);
  });

  it("goes to the work you pick", () => {
    openJump("item").querySelector('[data-work="issue:plan-1"]').click();
    expect(menu()).toBeNull();
    expect(location.hash).toBe("#/project/p1/issue/plan-1");
  });

  it("reaches the creates from the project half too, through the project you pick", () => {
    App.route = { name: "inbox" };
    toolbarRouteChanged();
    const popup = openJump("project");
    popup.querySelector('[data-project="p1"]').click();
    expect([...menu().querySelectorAll("[data-create]")].map((row) => row.dataset.create)).toEqual(["branch", "issue"]);
  });

  it("shuts on Escape", () => {
    openJump("project");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(menu()).toBeNull();
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
    const popup = openJump("item");
    expect(await churn(popup, () => refreshFeed())).toEqual([]);
  });

  it("redraws only the row that changed, and never the box being typed into", async () => {
    const popup = openJump("item");
    const filter = popup.querySelector(".tb-filter");
    const rows = [...popup.querySelectorAll("[data-work]")];
    const moved = { ...quiet.items[0], unread_count: 3 };
    const records = await churn(popup, () => {
      feed = { ...quiet, items: [quiet.items[1], moved] };
      return refreshFeed();
    });
    const branchRow = popup.querySelector('[data-work="branch:p1:build/login"]');
    expect(popup.querySelector(".tb-filter")).toBe(filter); // never replaced
    expect([...popup.querySelectorAll("[data-work]")]).toEqual(rows);
    expect(records.length).toBeGreaterThan(0);
    expect(records.every((record) => branchRow.contains(record.target))).toBe(true);
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
        { kind: "branch", project_id: "p2", project: "mascot", branch: "build/spike", title: "Mascot spike", resume_at: ago(10) },
      ],
    };
    await refreshFeed();
  });

  afterAll(() => {
    feed = quiet;
  });

  it("holds the pick while you stand on another project's branch and the feed ticks", async () => {
    openJump("project").querySelector('[data-project="p2"]').click();
    expect(menu().querySelector(".tb-scope span").textContent).toBe("mascot");
    await refreshFeed(); // two seconds later…
    await refreshFeed(); // …and two more
    expect(menu().querySelector(".tb-scope span").textContent).toBe("mascot");
    expect([...menu().querySelectorAll("[data-work]")].map((row) => row.querySelector(".mt").textContent)).toEqual(["build/spike"]);
  });

  it("still marks the picked project as the current one on the way back", async () => {
    openJump("project").querySelector('[data-project="p2"]').click();
    await refreshFeed();
    menu().querySelector("[data-projects]").click();
    expect(menu().querySelector(".mi.current .mt").textContent).toBe("mascot");
  });

  it("re-scopes to the project you navigate into", () => {
    openJump("project").querySelector('[data-project="p2"]').click();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    App.route = { name: "issue", projectId: "p1", id: "plan-1" };
    toolbarRouteChanged();
    expect([...openJump("item").querySelectorAll("[data-work]")].map((row) => row.querySelector(".mt").textContent)).toEqual([
      "Add a health endpoint",
      "build/login",
    ]);
  });
});

describe("creating from the menu", () => {
  // Both creates open the one create surface (core/createWork.js), on the
  // scoped project, on the tab that was picked; the menu is gone by then.
  it("opens the create modal on the Branch tab, scoped to the project the menu is on", () => {
    openJump("item").querySelector('[data-create="branch"]').click();
    expect(openCreateWork).toHaveBeenCalledWith({ projectId: "p1", projectName: "relaydb", kind: "branch" });
    expect(menu()).toBeNull();
  });

  it("opens it on the Issue tab for the issue create", () => {
    openJump("item").querySelector('[data-create="issue"]').click();
    expect(openCreateWork).toHaveBeenCalledWith({ projectId: "p1", projectName: "relaydb", kind: "issue" });
  });

  it("creates in the project you picked, not the one you are standing on", () => {
    openJump("project").querySelector('[data-project="p2"]').click();
    menu().querySelector('[data-create="branch"]').click();
    expect(openCreateWork).toHaveBeenCalledWith({ projectId: "p2", projectName: "mascot", kind: "branch" });
  });

  it("says so instead of opening anything when the device has no project", async () => {
    feed = { items: [], projects: [] };
    await refreshFeed();
    toolbarRouteChanged();
    openJump("item").querySelector('[data-create="branch"]').click();
    expect(openCreateWork).not.toHaveBeenCalled();
    expect(notifyError).toHaveBeenCalledWith("No project to create in.", expect.any(String));
    feed = savedFeed;
    await refreshFeed();
  });
});

describe("the unread counters on the two menus", () => {
  const quiet = feed;
  const badges = (selector) => [...menu().querySelectorAll(selector)].map((row) => (row.querySelector(".badge") || {}).textContent || "");

  beforeEach(async () => {
    feed = {
      ...quiet,
      items: [
        { ...quiet.items[0], unread: true, unread_count: 2 },
        { ...quiet.items[1], unread: true, unread_count: 3 },
        { kind: "branch", project_id: "p2", project: "mascot", branch: "build/spike", title: "", unread: true, unread_count: 4, resume_at: ago(10) },
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

  it("counts each branch and issue by its own unread, and leaves a read one bare", async () => {
    feed = { ...feed, items: [{ ...feed.items[0], unread: false, unread_count: 0 }, feed.items[1], feed.items[2]] };
    await refreshFeed();
    openJump("item");
    expect(badges("[data-work]")).toEqual(["3", ""]);
  });

  it("wears no counter anywhere once everything has been read", async () => {
    feed = quiet;
    await refreshFeed();
    openJump("project");
    expect(badges("[data-project]")).toEqual(["", ""]);
    openJump("item");
    expect(badges("[data-work]")).toEqual(["", ""]);
  });
});

describe("the ⋯", () => {
  it("carries what the tab row's right cluster used to", () => {
    bar().querySelector('[data-select="more"]').click();
    expect([...menu().querySelectorAll("[data-action]")].map((row) => row.dataset.action)).toEqual(["archive", "settings"]);
    menu().querySelector('[data-action="settings"]').click();
    expect(openProjectSettings).toHaveBeenCalledWith("p1");
  });

  it("sends Archive to the account page that owns it", () => {
    bar().querySelector('[data-select="more"]').click();
    menu().querySelector('[data-action="archive"]').click();
    expect(location.hash).toBe("#/account/archive");
  });
});
