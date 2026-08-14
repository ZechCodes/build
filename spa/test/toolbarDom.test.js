// @vitest-environment jsdom
// The view-area toolbar's wiring: the sentence it prints, the one menu both
// halves open, the two creates behind it, and the status on its right.

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
let subscriber = null;
const refreshFeed = vi.fn(async () => subscriber && subscriber(feed));
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    subscriber = fn;
    fn(feed);
    return () => {};
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
  if (!document.getElementById("shell")) document.body.innerHTML = bodyHtml;
  localStorage.clear();
  App.gated = false;
  App.route = { name: "branch", projectId: "p1", branch: "build/login", tab: "changes" };
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
  it("names the project, then the branch, and reports what the work is doing", () => {
    expect(names()).toEqual(["relaydb", "build/login"]);
    const status = document.getElementById("tb-status").textContent;
    expect(status).toContain("working 12m");
    expect(status).toContain("+42 −7");
  });

  it("names an issue by its title, with no diffstat to report", () => {
    App.route = { name: "issue", projectId: "p1", id: "plan-1" };
    toolbarRouteChanged();
    expect(names()).toEqual(["relaydb", "Add a health endpoint"]);
    expect(document.getElementById("tb-status").textContent.trim()).toBe("");
  });

  it("keeps the project selector on a route that is no work item", () => {
    App.route = { name: "inbox" };
    toolbarRouteChanged();
    expect(names()).toEqual(["relaydb"]);
    expect(bar().querySelector('[data-select="item"]')).toBeNull();
  });
});

describe("the one menu both halves open", () => {
  it("lists every project and the work inside the scoped one, with both creates", () => {
    const popup = openJump("project");
    expect([...popup.querySelectorAll("[data-project]")].map((row) => row.textContent.trim())).toEqual(["relaydb", "mascot"]);
    expect([...popup.querySelectorAll("[data-work]")].map((row) => row.querySelector(".mt").textContent)).toEqual([
      "Add a health endpoint",
      "build/login",
    ]);
    expect([...popup.querySelectorAll("[data-create]")].map((row) => row.dataset.create)).toEqual(["branch", "issue"]);
  });

  it("is the same menu from the item half", () => {
    const popup = openJump("item");
    expect(popup.querySelectorAll("[data-project]").length).toBe(2);
    expect(popup.querySelectorAll("[data-work]").length).toBe(2);
  });

  it("filters both halves with one query", () => {
    const popup = openJump("project");
    const filter = popup.querySelector(".tb-filter");
    filter.value = "login";
    filter.dispatchEvent(new Event("input"));
    expect([...menu().querySelectorAll("[data-work]")].map((row) => row.querySelector(".mt").textContent)).toEqual(["build/login"]);
    expect(menu().querySelectorAll("[data-project]").length).toBe(0);
  });

  it("re-scopes the work half to the project you pick, without leaving the page", () => {
    const popup = openJump("project");
    popup.querySelector('[data-project="p2"]').click();
    expect(menu()).toBeTruthy();
    expect(menu().querySelectorAll("[data-work]").length).toBe(0);
    expect(menu().textContent).toContain("Nothing here yet.");
    expect(location.hash).not.toContain("p2/branch");
  });

  it("goes to the work you pick", () => {
    openJump("project").querySelector('[data-work="issue:plan-1"]').click();
    expect(menu()).toBeNull();
    expect(location.hash).toBe("#/project/p1/issue/plan-1");
  });

  it("shuts on Escape", () => {
    openJump("project");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(menu()).toBeNull();
  });
});

describe("creating from the menu", () => {
  it("cuts a branch and opens it, echoing the branch the daemon will name", async () => {
    openJump("project").querySelector('[data-create="branch"]').click();
    const input = menu().querySelector("#tb-create-input");
    input.value = "Mascot Model Spike!";
    input.dispatchEvent(new Event("input"));
    expect(menu().querySelector("#tb-create-preview").textContent).toBe("build/mascot-model-spike");
    menu().querySelector("[data-create-go]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("worktree.create", { project_id: "p1", name: "Mascot Model Spike!" });
    expect(location.hash).toBe("#/project/p1/branch/build%2Fmascot-model-spike/changes");
    expect(menu()).toBeNull();
  });

  it("files an issue that starts nothing, and opens it", async () => {
    openJump("project").querySelector('[data-create="issue"]').click();
    const input = menu().querySelector("#tb-create-input");
    input.value = "Add a /health endpoint";
    input.dispatchEvent(new Event("input"));
    menu().querySelector("[data-create-go]").click();
    await flush();
    // Inert by contract: the record exists and no agent is dispatched until the
    // first message.
    expect(App.call).toHaveBeenCalledWith("issue.create", {
      goal: "Add a /health endpoint",
      project_id: "p1",
      dispatch: false,
    });
    expect(location.hash).toBe("#/project/p1/issue/plan-9");
  });

  it("says what went wrong without losing what was typed", async () => {
    App.call = vi.fn(async () => {
      throw new Error("a worktree named that already exists");
    });
    openJump("project").querySelector('[data-create="branch"]').click();
    const input = menu().querySelector("#tb-create-input");
    input.value = "scratch";
    input.dispatchEvent(new Event("input"));
    menu().querySelector("[data-create-go]").click();
    await flush();
    expect(menu().querySelector(".tb-create-error").textContent).toContain("already exists");
    expect(menu().querySelector("#tb-create-input").value).toBe("scratch");
  });

  it("refuses an empty answer instead of creating something unnamed", async () => {
    openJump("project").querySelector('[data-create="issue"]').click();
    menu().querySelector("[data-create-go]").click();
    await flush();
    expect(App.call).not.toHaveBeenCalledWith("issue.create", expect.anything());
    expect(menu().querySelector(".tb-create-error").textContent).toContain("Describe the issue");
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
