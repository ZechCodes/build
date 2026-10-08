// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "fake-indexeddb/auto";

const state = vi.hoisted(() => ({
  App: { route: {}, devices: [], deviceFilter: null },
  feed: { items: [], projects: [], workspaces: [], devices: {} },
  listeners: new Set(),
  go: vi.fn(),
  openWorkspaceReview: vi.fn(),
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
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: vi.fn() }));
vi.mock("../src/core/notify.js", () => ({ notifyError: vi.fn() }));
vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: () => ({ rpc: state.rpc }), whenGreeted: vi.fn() }));

import { initToolbar, stopToolbar, toolbarRouteChanged } from "../src/core/toolbar.js";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { rememberWorkspaceLockSupport } from "../src/core/workspaceLockSupport.js";
import { stampWorkspace } from "../src/core/feedMerge.js";

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
  state.rpc.mockClear();
});
afterEach(async () => { await stopToolbar(); });

describe("workspace jump-menu review entry", () => {
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

  it("keeps the entry tied to the standing project when the same menu switches project", async () => {
    await mount();
    await vi.waitFor(() => expect(reviewButton()).not.toBeNull());
    menu().querySelector("[data-projects]").click();
    await vi.waitFor(() => expect(menu()?.dataset.list).toBe("projects"));
    menu().querySelector('[data-project="device/other"]').click();
    await vi.waitFor(() => expect(menu()?.dataset.list).toBe("workspaces"));
    expect(reviewButton()).toBeNull();
    deliver([workspace()]);
    expect(reviewButton()).toBeNull();
  });

  it("does not borrow another workspace's active review or another device's capability", async () => {
    deliver([workspace({ active_review: activeReview({ workspace_id: "different" }) })]);
    await mount();
    await vi.waitFor(() => expect(reviewButton()?.textContent).toContain("Open review"));
    state.App.route = route({ deviceId: "other-device" });
    toolbarRouteChanged();
    await vi.waitFor(() => expect(reviewButton()).toBeNull());
    expect(state.rpc).not.toHaveBeenCalled();
  });
});
