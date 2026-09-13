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
const { App } = await import("../src/app.js");
const { initToolbar, stopToolbar, toolbarRouteChanged } = await import("../src/core/toolbar.js");
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const bar = () => document.querySelector("#toolbar .toolbar");
const menu = () => document.querySelector(".tbmenu");
const open = (selector) => { bar().querySelector(`[data-select="${selector}"]`).click(); return menu(); };

beforeEach(() => {
  stopToolbar();
  document.body.innerHTML = '<div id="toolbar"></div><div id="root"></div><div id="console-region"></div><div id="agent-rail"></div>';
  App.gated = true;
  App.route = { name: "branch", projectId: "p1", branch: "build/login", tab: "changes" };
  App.call = vi.fn(async (method) => method === "workspace.list" ? { workspaces: [workspace] } : method === "workspace.get" ? { workspace } : {});
  initToolbar();
  toolbarRouteChanged();
});
afterAll(stopToolbar);

describe("workspace toolbar", () => {
  it("keeps legacy deep-link identities readable without an active work menu", () => {
    expect([...bar().querySelectorAll(".tb-name")].map((node) => node.textContent)).toEqual(["relaydb", "build/login"]);
    expect(bar().querySelector('[data-select="item"]')).toBeNull();
  });
  it("keeps the legacy project selector functional and opens it projects-first", () => {
    const projects = open("project");
    expect(projects.dataset.list).toBe("projects");
    expect([...projects.querySelectorAll("[data-project]")].map((row) => row.querySelector(".mt").textContent)).toEqual(["relaydb", "mascot"]);
  });
  it("shows only the workspace switcher before its directory tabs", async () => {
    App.route = { name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" };
    toolbarRouteChanged();
    await flush();
    expect(bar().querySelector('[data-select="project"]')).toBeNull();
    expect(bar().querySelector('[data-select="workspace"] .tb-name').textContent).toBe("payment-work");
    expect([...bar().children].indexOf(bar().querySelector('[data-select="workspace"]')))
      .toBeLessThan([...bar().children].indexOf(bar().querySelector(".tb-directories")));
  });
  it("lists the current project's workspaces, then switches project in the same popup", async () => {
    App.route = { name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" };
    toolbarRouteChanged();
    await flush();
    const workspaces = open("workspace");
    expect([...workspaces.querySelectorAll("[data-workspace]")].map((row) => row.querySelector(".mt").textContent)).toEqual(["payment-work"]);
    expect(workspaces.querySelector("[data-projects]").textContent).toBe("Switch project");
    workspaces.querySelector("[data-projects]").click();
    const projects = menu();
    expect([...projects.querySelectorAll("[data-project]")].map((row) => row.textContent.trim())).toEqual(["relaydb", "mascot"]);
    projects.querySelector('[data-project="p1"]').click();
    await flush();
    expect([...menu().querySelectorAll("[data-workspace]")].map((row) => row.querySelector(".mt").textContent)).toEqual(["payment-work"]);
    expect(menu().querySelector("[data-work]")).toBeNull();
    expect(menu().querySelector('[data-create="workspace"]')).toBeTruthy();
  });
  it("keeps the active workspace visible while browsing another project and resets scope when reopened", async () => {
    const sandbox = { id: "ws-2", project_id: "p2", name: "prototype", status: "ready", directories: [] };
    App.call = vi.fn(async (method, params) => {
      if (method === "workspace.list") return { workspaces: params.project_id === "p2" ? [sandbox] : [workspace] };
      if (method === "workspace.get") return { workspace };
      return {};
    });
    App.route = { name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" };
    toolbarRouteChanged();
    await flush();

    open("workspace").querySelector("[data-projects]").click();
    menu().querySelector('[data-project="p2"]').click();
    expect(bar().querySelector('[data-select="workspace"] .tb-name').textContent).toBe("payment-work");
    expect([...bar().querySelectorAll("[data-directory]")].map((node) => node.textContent)).toEqual(["Frontend", "Design assets"]);
    await flush();
    expect([...menu().querySelectorAll("[data-workspace]")].map((row) => row.querySelector(".mt").textContent)).toEqual(["prototype"]);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    const reopened = open("workspace");
    expect(reopened.querySelector(".tb-scope > span").textContent).toBe("relaydb");
    expect([...reopened.querySelectorAll("[data-workspace]")].map((row) => row.querySelector(".mt").textContent)).toEqual(["payment-work"]);
  });
  it("rehydrates the active workspace when its project is selected from the popup", async () => {
    App.call = vi.fn(async (method) => {
      if (method === "workspace.list") return { workspaces: [{ id: "ws-1", project_id: "p1", name: "payment-work", status: "ready" }] };
      if (method === "workspace.get") return { workspace };
      return {};
    });
    App.route = { name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" };
    toolbarRouteChanged();
    await flush();

    open("workspace").querySelector("[data-projects]").click();
    menu().querySelector('[data-project="p1"]').click();
    await flush();

    expect(App.call).toHaveBeenCalledWith("workspace.get", { workspace_id: "ws-1" });
    expect([...bar().querySelectorAll("[data-directory]")].map((node) => node.textContent)).toEqual(["Frontend", "Design assets"]);
  });
  it("ignores a workspace list response overtaken by a newer project choice", async () => {
    let resolveSandbox;
    const sandboxAnswer = new Promise((resolve) => { resolveSandbox = resolve; });
    App.call = vi.fn(async (method, params) => {
      if (method === "workspace.list" && params.project_id === "p2") return sandboxAnswer;
      if (method === "workspace.list") return { workspaces: [workspace] };
      if (method === "workspace.get") return { workspace };
      return {};
    });
    App.route = { name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" };
    toolbarRouteChanged();
    await flush();

    open("workspace").querySelector("[data-projects]").click();
    menu().querySelector('[data-project="p2"]').click();
    menu().querySelector("[data-projects]").click();
    menu().querySelector('[data-project="p1"]').click();
    await flush();
    resolveSandbox({ workspaces: [{ id: "ws-2", project_id: "p2", name: "prototype" }] });
    await flush();

    expect(menu().querySelector(".tb-scope > span").textContent).toBe("relaydb");
    expect([...menu().querySelectorAll("[data-workspace]")].map((row) => row.querySelector(".mt").textContent)).toEqual(["payment-work"]);
  });
  it("offers workspace creation even when the project has no workspaces", async () => {
    App.call = vi.fn(async (method) => method === "workspace.list" ? { workspaces: [] } : {});
    App.route = { name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes" };
    toolbarRouteChanged();
    await flush();
    const workspaces = open("workspace");
    workspaces.querySelector("[data-projects]").click();
    menu().querySelector('[data-project="p1"]').click();
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
    expect(picker.getAttribute("aria-expanded")).toBe("true");
    expect([...menu().querySelectorAll("[data-menu-directory]")].map((node) => [node.textContent.trim(), node.classList.contains("current")])).toEqual([
      ["Frontend", true], ["Design assets", false],
    ]);
    expect(document.activeElement).toBe(menu().querySelector('[data-menu-directory="frontend"]'));
    expect(menu().querySelector('[data-menu-directory="frontend"]').getAttribute("aria-checked")).toBe("true");
    expect(menu().querySelector('[data-menu-directory="assets"]').getAttribute("aria-checked")).toBe("false");
    menu().querySelector('[data-menu-directory="assets"]').click();
    expect(App.route).toEqual({ name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "assets", tab: "files" });
  });
  it("leaves finish and contextual actions out of the navigation toolbar", () => {
    expect(bar().querySelector("#tb-verb").children).toHaveLength(0);
    expect(bar().querySelector('[data-select="more"]')).toBeNull();
  });
});
