// @vitest-environment jsdom
// The view-area toolbar's wiring: the sentence it prints, the menu each half
// opens, the two creates behind the work half, and the ⋯ on its right.

import { describe, it, expect, beforeEach, afterAll, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const NOW = Date.now();
const ago = (seconds) => new Date(NOW - seconds * 1000).toISOString();

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

const { App } = await import("../src/app.js");
const { initToolbar, openCreateFrom, stopToolbar, toolbarRouteChanged } = await import("../src/core/toolbar.js");

const flush = () => new Promise((done) => setTimeout(done, 0));
const bar = () => document.querySelector("#toolbar .toolbar");
const menu = () => document.querySelector(".tbmenu");
const names = () => [...bar().querySelectorAll(".tb-name")].map((name) => name.textContent);
const openJump = (which = "project") => {
  bar().querySelector(`[data-select="${which}"]`).click();
  return menu();
};

beforeEach(() => {
  if (!document.getElementById("shell")) document.body.innerHTML = bodyHtml;
  localStorage.clear();
  App.gated = false;
  App.route = { name: "branch", projectId: "p1", branch: "build/login", tab: "changes" };
  App.focusComposerOnMount = false;
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
  it("cuts a branch and opens it, echoing the branch the daemon will name", async () => {
    openJump("item").querySelector('[data-create="branch"]').click();
    const input = menu().querySelector("#tb-create-input");
    input.value = "Mascot Model Spike!";
    input.dispatchEvent(new Event("input"));
    expect(menu().querySelector("#tb-create-preview").textContent).toBe("build/mascot-model-spike");
    menu().querySelector("[data-create-go]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("worktree.create", { project_id: "p1", name: "Mascot Model Spike!" });
    expect(location.hash).toBe("#/project/p1/branch/build%2Fmascot-model-spike/changes");
    expect(menu()).toBeNull();
    // The branch view this navigation lands on reads this to focus the rail's
    // composer the moment it exists — a branch this fresh has nobody in it yet.
    expect(App.focusComposerOnMount).toBe(true);
  });

  it("files an issue that starts nothing, and opens it", async () => {
    openJump("item").querySelector('[data-create="issue"]').click();
    const input = menu().querySelector("#tb-create-input");
    input.value = "Add a /health endpoint";
    input.dispatchEvent(new Event("input"));
    menu().querySelector("[data-create-go]").click();
    await flush();
    // Issue creation is not what was asked for this — only a branch cut from
    // this form arms the composer autofocus.
    expect(App.focusComposerOnMount).toBe(false);
    // Inert by contract: the record exists and no agent is dispatched until the
    // first message.
    expect(App.call).toHaveBeenCalledWith("issue.create", {
      goal: "Add a /health endpoint",
      project_id: "p1",
      dispatch: false,
    });
    expect(location.hash).toBe("#/project/p1/issue/plan-9");
  });

  it("carries the harness picker on the issue create — the same panel compose asks with", async () => {
    App.modelCatalog = {
      default_provider: "claude",
      providers: [
        { id: "claude", label: "Claude Code", models: [{ id: "opus", label: "Opus", supports_effort: true }], efforts: ["low"] },
      ],
    };
    openJump("item").querySelector('[data-create="issue"]').click();
    const input = menu().querySelector("#tb-create-input");
    input.value = "Add a /health endpoint";
    input.dispatchEvent(new Event("input"));
    menu().querySelector("[data-agent-choice-toggle]").click();
    const model = menu().querySelector("#tb-choice-model");
    model.value = "opus";
    model.dispatchEvent(new Event("change", { bubbles: true }));
    menu().querySelector("[data-create-go]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("issue.create", {
      goal: "Add a /health endpoint",
      project_id: "p1",
      dispatch: false,
      provider: "claude",
      model: "opus",
    });
  });

  it("asks no harness question of a branch create, which starts no agent to answer for", () => {
    openJump("item").querySelector('[data-create="branch"]').click();
    expect(menu().querySelector("[data-agent-choice-toggle]")).toBeNull();
  });

  it("says what went wrong without losing what was typed", async () => {
    App.call = vi.fn(async () => {
      throw new Error("a worktree named that already exists");
    });
    openJump("item").querySelector('[data-create="branch"]').click();
    const input = menu().querySelector("#tb-create-input");
    input.value = "scratch";
    input.dispatchEvent(new Event("input"));
    menu().querySelector("[data-create-go]").click();
    await flush();
    expect(menu().querySelector(".tb-create-error").textContent).toContain("already exists");
    expect(menu().querySelector("#tb-create-input").value).toBe("scratch");
  });

  it("refuses an empty answer instead of creating something unnamed", async () => {
    openJump("item").querySelector('[data-create="issue"]').click();
    menu().querySelector("[data-create-go]").click();
    await flush();
    expect(App.call).not.toHaveBeenCalledWith("issue.create", expect.anything());
    expect(menu().querySelector(".tb-create-error").textContent).toContain("Describe the issue");
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

// ---- the create, opened from the rail ---------------------------------------
// A project block on the rail offers the same two creates. They open the same
// form in the same popup, scoped to the block's project, with no list behind
// them to come back to.

describe("creating from the rail", () => {
  const anchor = () => document.getElementById("inbox-collapse");

  it("opens the form scoped to the named project, and cancel shuts the popup whole", () => {
    openCreateFrom(anchor(), { projectId: "p2", kind: "branch", navigate: vi.fn() });
    expect(menu().querySelector(".tb-create-head").textContent).toBe("New branch in mascot");
    menu().querySelector("[data-create-cancel]").click();
    expect(menu()).toBeNull();
  });

  it("cuts the branch in that project and hands what it made to the caller's navigate", async () => {
    const navigate = vi.fn();
    openCreateFrom(anchor(), { projectId: "p2", kind: "branch", navigate });
    const input = menu().querySelector("#tb-create-input");
    input.value = "Mascot Model Spike!";
    input.dispatchEvent(new Event("input"));
    menu().querySelector("[data-create-go]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("worktree.create", { project_id: "p2", name: "Mascot Model Spike!" });
    expect(navigate).toHaveBeenCalledWith({ name: "branch", projectId: "p1", branch: "build/mascot-model-spike", tab: "changes" });
    expect(menu()).toBeNull();
  });

  it("files an issue in that project", async () => {
    const navigate = vi.fn();
    openCreateFrom(anchor(), { projectId: "p2", kind: "issue", navigate });
    expect(menu().querySelector(".tb-create-head").textContent).toBe("New issue in mascot");
    const input = menu().querySelector("#tb-create-input");
    input.value = "Add a health endpoint";
    input.dispatchEvent(new Event("input"));
    menu().querySelector("[data-create-go]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith(
      "issue.create",
      expect.objectContaining({ goal: "Add a health endpoint", project_id: "p2", dispatch: false }),
    );
    expect(navigate).toHaveBeenCalledWith({ name: "issue", projectId: "p1", id: "plan-9" });
  });

  it("replaces a jump menu that was already open", () => {
    openJump("project");
    expect(menu()).toBeTruthy();
    openCreateFrom(anchor(), { projectId: "p1", kind: "branch", navigate: vi.fn() });
    expect(document.querySelectorAll(".tbmenu").length).toBe(1);
    expect(menu().querySelector(".tb-create-head").textContent).toBe("New branch in relaydb");
    menu().querySelector("[data-create-cancel]").click();
  });
});
