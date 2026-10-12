// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "fake-indexeddb/auto";

const state = vi.hoisted(() => ({
  App: { route: {}, devices: [], deviceFilter: null },
  feed: { items: [], projects: [], workspaces: [], devices: {} },
  listeners: new Set(),
  go: vi.fn(),
  openWorkspaceReview: vi.fn(),
  openCreateWork: vi.fn(),
  rpc: vi.fn(),
}));
vi.mock("../src/app.js", () => ({ App: state.App, go: state.go }));
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed(listener) {
    state.listeners.add(listener);
    listener(state.feed);
    return () => state.listeners.delete(listener);
  },
}));
vi.mock("../src/core/workspaceReviewEntry.js", () => ({ openWorkspaceReview: state.openWorkspaceReview }));
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: state.openCreateWork }));
vi.mock("../src/core/notify.js", () => ({ notifyError: vi.fn() }));
vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: () => ({ rpc: state.rpc }), whenGreeted: vi.fn() }));

import { initToolbar, stopToolbar, toolbarRouteChanged } from "../src/core/toolbar.js";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { rememberWorkspaceLockSupport } from "../src/core/workspaceLockSupport.js";
import { stampWorkspace } from "../src/core/feedMerge.js";
import { hashFromRoute } from "../src/core/router.js";
import { rememberProjectRailRoute } from "../src/core/projectRailState.js";

const route = (extra = {}) => ({ name: "workspace", deviceId: "device", projectId: "project", workspaceId: "workspace", ...extra });
const workspace = (extra = {}) => ({ id: "workspace", project_id: "project", name: "Feature work", status: "ready", locked: false, ...extra });
const activeReview = (extra = {}) => ({ task_id: "task", workspace_id: "workspace", version: 2, status: "open", ...extra });
const menu = () => document.querySelector(".tbmenu");
const reviewButton = () => menu()?.querySelector("[data-workspace-review]");
const support = (reviews) => rememberReviewSupport("device", { reviews });
const deliver = (workspaces) => {
  const rows = workspaces.map((row) => stampWorkspace(row, "device"));
  state.feed = { ...state.feed, workspaces: rows, devices: { device: { workspaces: rows } } };
  for (const listener of state.listeners) listener(state.feed);
};
const openJump = async () => {
  document.querySelector('[data-select="workspace"]').click();
  expect(document.activeElement).toBe(menu()?.querySelector(".tb-filter"));
  await vi.waitFor(() => expect(menu()?.dataset.list).toBe("workspaces"));
};
const mount = async (reviews = { pullRequests: true, open: true, get: true }) => {
  await support(reviews);
  await initToolbar();
  await openJump();
};

beforeEach(async () => {
  await stopToolbar();
  await wipeCache();
  await wipeUiRecords();
  localStorage.clear();
  sessionStorage.clear();
  document.body.innerHTML = '<div id="toolbar"></div>';
  state.App.route = route();
  state.App.devices = [{ id: "device", name: "Laptop" }];
  state.App.deviceFilter = null;
  state.feed = {
    items: [],
    projects: [
      { id: "project", deviceId: "device", projectKey: "device/project", name: "Build" },
      { id: "other", deviceId: "device", projectKey: "device/other", name: "Other" },
    ],
    workspaces: [], devices: {},
  };
  deliver([workspace()]);
  state.go.mockClear();
  state.openWorkspaceReview.mockClear();
  state.openCreateWork.mockClear();
  state.rpc.mockClear();
});
afterEach(async () => { await stopToolbar(); });

describe("workspace jump-menu review entry", () => {
  it("keeps review and New workspace in the local actions footer only", async () => {
    await mount();
    await vi.waitFor(() => expect(reviewButton()).not.toBeNull());
    expect(menu().querySelector('[data-create="workspace"]')).not.toBeNull();
    expect(menu().textContent).toContain("Workspace actions");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await vi.waitFor(() => expect(menu()).toBeNull());
    document.querySelector('[data-select="project"]').click();
    expect(document.activeElement).toBe(menu()?.querySelector(".tb-filter"));
    await vi.waitFor(() => expect(menu()?.dataset.list).toBe("projects"));
    expect(reviewButton()).toBeNull();
    expect(menu().querySelector('[data-create="workspace"]')).toBeNull();
    expect(state.rpc).not.toHaveBeenCalled();
  });

  it.each([
    {}, { get: true, snapshot: true }, { pullRequests: true }, { open: true },
    { pullRequests: true, update: true },
  ])("does not offer PR creation without its exact cached capability pair (%j)", async (reviews) => {
    await mount(reviews);
    expect(reviewButton()).toBeNull();
    expect(state.rpc).not.toHaveBeenCalled();
  });

  it("opens the shared form from the workspace menu using cached support alone", async () => {
    await mount();
    await vi.waitFor(() => expect(reviewButton()?.textContent).toContain("Open review"));
    expect(document.querySelector('#toolbar [data-workspace-review]')).toBeNull();
    expect(document.querySelector('#toolbar #tb-verb')).toBeNull();
    reviewButton().click();
    await vi.waitFor(() => expect(state.openWorkspaceReview).toHaveBeenCalledExactlyOnceWith({
      deviceId: "device", projectId: "project", workspaceId: "workspace", navigate: state.go,
    }));
    await vi.waitFor(() => expect(menu()).toBeNull());
    expect(state.rpc).not.toHaveBeenCalled();
  });

  it("links to the cached active PR's task without needing the open mutation capability", async () => {
    deliver([workspace({ active_review: activeReview() })]);
    await mount({ pullRequests: true, get: true });
    await vi.waitFor(() => expect(reviewButton()?.textContent).toContain("View pull request"));
    reviewButton().click();
    expect(state.go).toHaveBeenCalledExactlyOnceWith({ name: "trackerTask", deviceId: "device", projectId: "project", taskId: "task" });
    expect(state.openWorkspaceReview).not.toHaveBeenCalled();
    expect(state.rpc).not.toHaveBeenCalled();
  });

  it.each([{ get: true }, { pullRequests: true }, { pullRequests: true, open: true }])("gates the active PR link on get and pullRequests (%j)", async (reviews) => {
    deliver([workspace({ active_review: activeReview() })]);
    await mount(reviews);
    expect(reviewButton()).toBeNull();
  });

  it("updates the open menu from capability and workspace cache deliveries while retaining focus and lock", async () => {
    await rememberWorkspaceLockSupport("device", { workspaces: { setLocked: true } });
    await mount({ pullRequests: true });
    await vi.waitFor(() => expect(document.querySelector("[data-workspace-lock]")).not.toBeNull());
    const heldMenu = menu();
    const filter = heldMenu.querySelector(".tb-filter");
    const lock = document.querySelector("[data-workspace-lock]");
    filter.value = "Feature";
    filter.dispatchEvent(new Event("input", { bubbles: true }));
    filter.focus();
    filter.setSelectionRange(2, 4);
    await support({ pullRequests: true, get: true, open: true });
    await vi.waitFor(() => expect(reviewButton()?.textContent).toContain("Open review"));
    const entry = reviewButton();
    deliver([workspace({ active_review: activeReview(), locked: true })]);
    await vi.waitFor(() => expect(reviewButton()?.textContent).toContain("View pull request"));
    expect(reviewButton()).toBe(entry);
    expect(menu()).toBe(heldMenu);
    expect(menu().querySelector(".tb-filter")).toBe(filter);
    expect(filter.value).toBe("Feature");
    expect(document.activeElement).toBe(filter);
    expect([filter.selectionStart, filter.selectionEnd]).toEqual([2, 4]);
    expect(document.querySelector("[data-workspace-lock]")).toBe(lock);
    expect(lock.getAttribute("aria-pressed")).toBe("true");
    await support({ get: true, snapshot: true });
    await vi.waitFor(() => expect(reviewButton()).toBeNull());
    expect(menu()).toBe(heldMenu);
    expect(state.rpc).not.toHaveBeenCalled();
  });

  it("removes local workspace actions when the same dialog widens globally", async () => {
    await mount();
    await vi.waitFor(() => expect(reviewButton()).not.toBeNull());
    const dialog = menu();
    menu().querySelector("[data-projects]").click();
    await vi.waitFor(() => expect(menu()?.dataset.list).toBe("projects"));
    expect(menu()).toBe(dialog);
    expect(menu().querySelector("[data-workspace-review], [data-create], [data-manage-workspaces]")).toBeNull();
    expect(menu().querySelector('[data-project="device/other"]:not([data-visit])').tagName).toBe("A");
    deliver([workspace()]);
    expect(reviewButton()).toBeNull();
    expect(state.rpc).not.toHaveBeenCalled();
  });

  it("does not borrow another workspace's active review or another device's capability", async () => {
    deliver([workspace({ active_review: activeReview({ workspace_id: "different" }) })]);
    await mount();
    await vi.waitFor(() => expect(reviewButton()?.textContent).toContain("Open review"));
    state.App.route = route({ deviceId: "other-device" });
    toolbarRouteChanged();
    await vi.waitFor(() => expect(menu()).toBeNull());
    await openJump();
    expect(reviewButton()).toBeNull();
    expect(menu().querySelector(".tb-scope > span").textContent).not.toBe("Build");
    expect(state.rpc).not.toHaveBeenCalled();
  });
});

describe("current breadcrumb and picker destinations", () => {
  it.each(["link", "click"])("returns to the URL project's saved face when only its child roster identifies it (%s)", async (activation) => {
    const saved = { name: "project", deviceId: "device", projectId: "project", tab: "files", sourceId: "docs", file: "README.md", line: 12 };
    await rememberProjectRailRoute(saved);
    state.feed = { ...state.feed, projects: [state.feed.projects[1]] };
    deliver([workspace()]);
    await initToolbar();
    await openJump();
    const returned = menu().querySelector("[data-project-page]");
    expect(returned).not.toBeNull();
    if (activation === "link") {
      await vi.waitFor(() => expect(returned.getAttribute("href")).toBe(hashFromRoute(saved)));
    } else {
      returned.click();
      await vi.waitFor(() => expect(state.go).toHaveBeenCalledExactlyOnceWith(saved, { newEntry: true }));
      expect(menu()).toBeNull();
    }
    expect(state.openCreateWork).not.toHaveBeenCalled();
    expect(state.rpc).not.toHaveBeenCalled();
  });

  it.each(["list", "create", "manage"])("does not borrow the first listed project when the URL project is missing (%s)", async (action) => {
    state.App.route = route({ projectId: "missing", workspaceId: "missing-work" });
    state.feed = { ...state.feed, projects: [state.feed.projects[1]] };
    deliver([workspace({ id: "foreign-work", project_id: "other", name: "Other project's checkout" })]);
    await initToolbar();
    await openJump();
    if (action === "list") {
      expect(menu().querySelector('[data-workspace="device/foreign-work"]')).toBeNull();
      expect(menu().textContent).not.toContain("Other project's checkout");
      expect(menu().querySelector(".tb-scope > span").textContent).not.toBe("Other");
    } else {
      menu().querySelector(action === "create" ? '[data-create="workspace"]' : "[data-manage-workspaces]").click();
      await vi.waitFor(() => expect(menu()).toBeNull());
      expect(state.openCreateWork).not.toHaveBeenCalled();
      expect(state.go).not.toHaveBeenCalled();
    }
    expect(state.rpc).not.toHaveBeenCalled();
  });

  it("moves ArrowDown from search to the first destination rather than Close", async () => {
    await initToolbar();
    await openJump();
    const filter = menu().querySelector(".tb-filter");
    const first = menu().querySelector('.tbmenu-list a[href]');
    filter.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement).toBe(first);
  });

  it("cycles ShiftTab from search and Tab from the last action within the dialog", async () => {
    await initToolbar();
    await openJump();
    const filter = menu().querySelector(".tb-filter");
    const close = menu().querySelector("[data-menu-close]");
    expect(document.activeElement).toBe(filter);
    filter.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(close);
    const last = menu().querySelector('[data-create="workspace"]');
    last.focus();
    last.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(close);
  });

  it("Enter from search activates Current without calling go", async () => {
    state.App.route = { name: "project", deviceId: "device", projectId: "project", tab: "files", sourceId: "docs", file: "README.md" };
    await initToolbar();
    await openJump();
    const filter = menu().querySelector(".tb-filter");
    expect(menu().querySelector('.tbmenu-list a[href]').classList.contains("current")).toBe(true);
    filter.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await vi.waitFor(() => expect(menu()).toBeNull());
    expect(state.go).not.toHaveBeenCalled();
  });

  it("Enter from filtered search opens the first matching sibling", async () => {
    deliver([workspace(), workspace({ id: "sibling", name: "Other checkout" })]);
    await initToolbar();
    await openJump();
    const filter = menu().querySelector(".tb-filter");
    filter.value = "Other checkout";
    filter.dispatchEvent(new Event("input"));
    expect(menu().querySelector('.tbmenu-list a[href]').dataset.workspace).toBe("device/sibling");
    filter.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await vi.waitFor(() => expect(state.go).toHaveBeenCalledExactlyOnceWith({
      name: "workspace", deviceId: "device", projectId: "project", workspaceId: "sibling", tab: "changes",
    }, { newEntry: true }));
    expect(menu()).toBeNull();
  });

  it.each([
    { name: "workspace", deviceId: "device", projectId: "project", workspaceId: "workspace", tab: "files", sourceId: "docs", file: "README.md", line: 12 },
    { name: "workspace", deviceId: "device", projectId: "project", workspaceId: "workspace", tab: "tasks", taskId: "task", view: "board" },
    { name: "project", deviceId: "device", projectId: "project", tab: "files", sourceId: "docs", file: "README.md", line: 12 },
  ])("closes a Current picker row without navigating or dropping context (%j)", async (standing) => {
    state.App.route = standing;
    const hash = hashFromRoute(standing);
    history.replaceState(null, "", hash);
    await initToolbar();
    await openJump();
    const current = menu().querySelector(standing.name === "workspace" ? '[data-workspace="device/workspace"]' : "[data-project-page]");
    expect(current.classList.contains("current")).toBe(true);
    current.click();
    await vi.waitFor(() => expect(menu()).toBeNull());
    expect(state.go).not.toHaveBeenCalled();
    expect(state.App.route).toBe(standing);
    expect(location.hash).toBe(hash);
    expect(state.rpc).not.toHaveBeenCalled();
  });

  it.each([
    { name: "project", deviceId: "device", projectId: "project", tab: "files", sourceId: "docs", file: "README.md", line: 12 },
    { name: "trackerTask", deviceId: "device", projectId: "project", taskId: "task", commentId: "comment" },
    { name: "workspace", deviceId: "device", projectId: "project", workspaceId: "workspace", tab: "tasks", taskId: "task", view: "board" },
    { name: "branch", deviceId: "device", projectId: "project", branch: "build/fix/navigation", tab: "files", file: "README.md", line: 12 },
  ])("makes the current leaf an exact self-link and skips go (%j)", async (standing) => {
    state.App.route = standing;
    const hash = hashFromRoute(standing);
    history.replaceState(null, "", hash);
    await initToolbar();
    const current = document.querySelector('#toolbar a[aria-current="page"]');
    expect(current).not.toBeNull();
    expect(current.getAttribute("href")).toBe(hash);
    current.click();
    await Promise.resolve();
    expect(state.go).not.toHaveBeenCalled();
    expect(location.hash).toBe(hash);
    expect(state.rpc).not.toHaveBeenCalled();
    if (standing.name === "branch") expect(current.textContent).toBe("build/fix/navigation");
  });
});
