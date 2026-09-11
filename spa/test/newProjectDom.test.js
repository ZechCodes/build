// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
const { App, openBrowser } = vi.hoisted(() => ({ App: {}, openBrowser: vi.fn() }));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/sheets/browser.js", () => ({ openBrowser }));
import { openNewRepo } from "../src/sheets/newRepo.js";
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
beforeEach(() => {
  vi.resetAllMocks(); document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  App.call = vi.fn(async (method) => method === "settings.get" ? { projects_dir: "/projects" } : { project_id: "p1" });
  App.session = { deviceId: "one" };
});

it("opens on the accessible multi-source project tab", () => {
  openNewRepo();
  expect([...document.querySelectorAll('[role="tab"]')].map((node) => node.textContent)).toEqual(["From sources", "Empty repository"]);
  expect(document.querySelector('[role="tab"]').getAttribute("aria-selected")).toBe("true");
  expect(document.querySelector("legend").textContent).toBe("Workspace folders");
});

it("creates one project from mixed local and remote sources", async () => {
  const done = vi.fn(); openNewRepo(done); document.querySelector("#nrname").value = " platform ";
  document.querySelector("#nraddfolder").click(); await flush(); openBrowser.mock.calls[0][0].onChoose("/projects/api");
  document.querySelector("#nraddremote").click(); const remote = document.querySelector("[data-source-value]");
  remote.value = " https://github.com/acme/web.git "; remote.dispatchEvent(new Event("input"));
  const branch = document.querySelectorAll("[data-source-branch]")[1]; branch.value = " trunk "; branch.dispatchEvent(new Event("input"));
  document.querySelector("#nrdo").click(); await flush();
  expect(App.call).toHaveBeenCalledWith("project.create", { name: "platform", sources: [
    { path: "/projects/api", name: "api" }, { remote: "https://github.com/acme/web.git", name: "web", base_branch: "trunk" },
  ] });
  expect(done).toHaveBeenCalledWith({ project_id: "p1" });
});

it("generates stable unique mount names and omits a removed source", async () => {
  openNewRepo(); document.querySelector("#nrname").value = "suite";
  for (let i = 0; i < 2; i += 1) { document.querySelector("#nraddfolder").click(); await flush(); openBrowser.mock.calls[i][0].onChoose(`/where${i}/api`); }
  expect([...document.querySelectorAll("[data-source-name]")].map((node) => node.value)).toEqual(["api", "api-2"]);
  document.querySelector("[data-remove-source]").click(); document.querySelector("#nrdo").click(); await flush();
  expect(App.call).toHaveBeenCalledWith("project.create", { name: "suite", sources: [{ path: "/where1/api", name: "api-2" }] });
});

it("rejects duplicate mount names and focuses the duplicate", () => {
  openNewRepo(); document.querySelector("#nrname").value = "suite";
  document.querySelector("#nraddremote").click(); document.querySelector("#nraddremote").click();
  [...document.querySelectorAll("[data-source-value]")].forEach((node, index) => { node.value = `https://host/repo-${index}.git`; node.dispatchEvent(new Event("input")); });
  [...document.querySelectorAll("[data-source-name]")].forEach((node) => { node.value = " API "; node.dispatchEvent(new Event("input")); });
  document.querySelector("#nrdo").click();
  expect(document.querySelector("#nrerr").textContent).toContain("must be unique");
  expect(document.activeElement).toBe(document.querySelectorAll("[data-source-name]")[1]); expect(App.call).not.toHaveBeenCalled();
});

it("ignores a stale browser choice and preserves empty-repository creation", async () => {
  openNewRepo(); document.querySelector("#nrname").value = "docs"; document.querySelector("#nraddfolder").click(); await flush();
  const choose = openBrowser.mock.calls[0][0].onChoose; document.querySelector("#nrback").click(); choose("/stale");
  expect(document.querySelector("#nrsources").textContent).toContain("No folder selected");
  document.querySelector('[data-project-tab="empty"]').click(); document.querySelector("#nrremote").value = "origin"; document.querySelector("#nrdo").click(); await flush();
  expect(App.call).toHaveBeenCalledWith("project.create", { name: "docs", remote: "origin" });
});

it("guards device changes and newer sheets", async () => {
  openNewRepo(); document.querySelector('[data-project-tab="empty"]').click(); document.querySelector("#nrname").value = "docs";
  const original = App.call; App.session = { deviceId: "two" }; App.call = vi.fn(); document.querySelector("#nrdo").click(); await flush();
  expect(original).not.toHaveBeenCalled(); expect(document.querySelector("#nrerr").textContent).toContain("device changed");
});
