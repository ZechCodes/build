// @vitest-environment jsdom
import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";
let feed = { items: [{ kind: "branch", project_id: "p1", project: "relaydb", branch: "build/login" }], projects: [{ id: "p1", name: "relaydb" }, { id: "p2", name: "mascot" }] };
const workspace = { id: "ws-1", project_id: "p1", name: "payment-work", status: "ready", directories: [
  { source_id: "frontend", name: "Frontend", is_git: true }, { source_id: "assets", name: "Design assets", is_git: false },
] };
const subscribers = new Set();
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => { subscribers.add(fn); fn(feed); return () => subscribers.delete(fn); },
  startFeed() {}, stopFeed() {}, refreshFeed: async () => subscribers.forEach((fn) => fn(feed)), primaryRunIdFor: () => null,
}));
const openProjectSettings = vi.fn();
vi.mock("../src/sheets/projectSettings.js", () => ({ openProjectSettings }));
const { App } = await import("../src/app.js");
const { initToolbar, stopToolbar, toolbarRouteChanged } = await import("../src/core/toolbar.js");
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const bar = () => document.querySelector("#toolbar .toolbar");
const menu = () => document.querySelector(".tbmenu");
const open = (selector) => { bar().querySelector(`[data-select="${selector}"]`).click(); return menu(); };

beforeEach(() => {
  document.body.innerHTML = '<div id="toolbar"></div><div id="root"></div><div id="console-region"></div><div id="agent-rail"></div>';
  App.gated = true;
  App.route = { name: "branch", projectId: "p1", branch: "build/login", tab: "changes" };
  App.call = vi.fn(async (method) => method === "workspace.list" ? { workspaces: [workspace] } : method === "workspace.get" ? { workspace } : {});
  openProjectSettings.mockClear();
  initToolbar();
  toolbarRouteChanged();
});
afterAll(stopToolbar);

describe("workspace toolbar", () => {
  it("keeps legacy deep-link identities readable without an active work menu", () => {
    expect([...bar().querySelectorAll(".tb-name")].map((node) => node.textContent)).toEqual(["relaydb", "build/login"]);
    expect(bar().querySelector('[data-select="item"]')).toBeNull();
  });
  it("lists projects first, then the selected project's workspaces", async () => {
    const projects = open("project");
    expect([...projects.querySelectorAll("[data-project]")].map((row) => row.textContent.trim())).toEqual(["relaydb", "mascot"]);
    projects.querySelector('[data-project="p1"]').click();
    await flush();
    expect([...menu().querySelectorAll("[data-workspace]")].map((row) => row.querySelector(".mt").textContent)).toEqual(["payment-work"]);
    expect(menu().querySelector("[data-work]")).toBeNull();
    expect(menu().querySelector('[data-create="workspace"]')).toBeTruthy();
  });
  it("offers workspace creation even when the project has no workspaces", async () => {
    App.call = vi.fn(async (method) => method === "workspace.list" ? { workspaces: [] } : {});
    const projects = open("project");
    projects.querySelector('[data-project="p1"]').click();
    await flush();
    expect(menu().querySelector('[data-create="workspace"]')).toBeTruthy();
    menu().querySelector('[data-create="workspace"]').click();
    expect(document.querySelector("#create-scrim h3").textContent).toBe("New workspace in relaydb");
  });
  it("shows directory tabs and opens ordinary directories in Files", async () => {
    App.route = { name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" };
    toolbarRouteChanged();
    await flush();
    expect([...bar().querySelectorAll("[data-directory]")].map((node) => [node.textContent, node.getAttribute("aria-selected")])).toEqual([
      ["Frontend", "true"], ["Design assets", "false"],
    ]);
    bar().querySelector('[data-directory="assets"]').click();
    expect(App.route).toEqual({ name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "assets", tab: "files" });
  });
  it("collapses directories into a phone menu without changing directory routing", async () => {
    App.route = { name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" };
    toolbarRouteChanged();
    await flush();
    const picker = bar().querySelector('[data-select="directory"]');
    expect(picker.textContent.trim()).toBe("Frontend▾");
    picker.click();
    expect([...menu().querySelectorAll("[data-menu-directory]")].map((node) => [node.textContent.trim(), node.classList.contains("current")])).toEqual([
      ["Frontend", true], ["Design assets", false],
    ]);
    expect(document.activeElement).toBe(menu().querySelector('[data-menu-directory="frontend"]'));
    expect(menu().querySelector('[data-menu-directory="frontend"]').getAttribute("aria-checked")).toBe("true");
    expect(menu().querySelector('[data-menu-directory="assets"]').getAttribute("aria-checked")).toBe("false");
    menu().querySelector('[data-menu-directory="assets"]').click();
    expect(App.route).toEqual({ name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "assets", tab: "files" });
  });
  it("keeps Archive and project settings in More", () => {
    open("more");
    expect([...menu().querySelectorAll("[data-action]")].map((row) => row.dataset.action)).toEqual(["archive", "settings"]);
    menu().querySelector('[data-action="settings"]').click();
    expect(openProjectSettings).toHaveBeenCalledWith("p1");
  });
});
