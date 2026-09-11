// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
const { App, openBrowser } = vi.hoisted(() => ({ App: {}, openBrowser: vi.fn() }));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/sheets/browser.js", () => ({ openBrowser }));
import { openNewRepo } from "../src/sheets/newRepo.js";
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
beforeEach(() => {
  vi.resetAllMocks();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  App.call = vi.fn(async (method) => method === "settings.get" ? { projects_dir: "/projects" } : { project_id: "p1" });
  App.session = { deviceId: "one", call: App.call };
});
it("opens on the create tab with persistent accessible project-source tabs", () => {
  openNewRepo();
  const tabs = [...document.querySelectorAll('[role="tab"]')];
  expect(tabs.map((tab) => tab.textContent)).toEqual(["Create new", "Existing folder"]);
  expect(tabs.map((tab) => tab.getAttribute("aria-selected"))).toEqual(["true", "false"]);
  expect(document.querySelector("#nrname")).not.toBeNull();
  expect(document.querySelector("#nrback")).toBeNull();
  expect(document.querySelector("#nrloc, #nrbrowse, #nrbranch")).toBeNull();
});
it("creates using only the name and optional remote in the device default directory", async () => {
  const done = vi.fn();
  openNewRepo(done);
  document.querySelector("#nrname").value = "my-project";
  document.querySelector("#nrremote").value = "git@github.com:example/project.git";
  document.querySelector("#nrdo").click();
  await flush();
  expect(App.call).toHaveBeenCalledWith("project.create", { name: "my-project", remote: "git@github.com:example/project.git" });
  expect(done).toHaveBeenCalledWith({ project_id: "p1" });
  expect(document.querySelector("#scrim").classList.contains("show")).toBe(false);
});
it("starts existing-folder browsing at the configured folder and permits non-Git folders", async () => {
  openNewRepo();
  document.querySelector('[data-project-tab="existing"]').click();
  await flush();
  expect(document.querySelectorAll('[role="tab"]')).toHaveLength(2);
  expect(openBrowser).toHaveBeenCalledWith(expect.objectContaining({ startPath: "/projects", gitOnly: false, callRpc: expect.any(Function), container: expect.any(HTMLElement) }));
  openBrowser.mock.calls[0][0].onChoose("/projects/docs");
  expect(document.querySelector("#nrexistingpath").textContent).toBe("/projects/docs");
  document.querySelector("#nrdo").click();
  await flush();
  expect(App.call).toHaveBeenCalledWith("project.add", { path: "/projects/docs" });
});
it("preserves create and existing-folder drafts when switching tabs", async () => {
  openNewRepo();
  document.querySelector("#nrname").value = "draft-name";
  document.querySelector("#nrremote").value = "draft-remote";
  document.querySelector('[data-project-tab="existing"]').click();
  await flush();
  openBrowser.mock.calls[0][0].onChoose("/projects/draft-folder");
  document.querySelector('[data-project-tab="create"]').click();
  expect(document.querySelector("#nrname").value).toBe("draft-name");
  expect(document.querySelector("#nrremote").value).toBe("draft-remote");
  document.querySelector('[data-project-tab="existing"]').click();
  expect(document.querySelector("#nrexistingpath").textContent).toBe("/projects/draft-folder");
});
it("ignores a folder choice from a browser after switching tabs", async () => {
  openNewRepo();
  document.querySelector('[data-project-tab="existing"]').click();
  await flush();
  const choose = openBrowser.mock.calls[0][0].onChoose;
  document.querySelector('[data-project-tab="create"]').click();
  choose("/projects/stale");
  document.querySelector('[data-project-tab="existing"]').click();
  expect(document.querySelector("#nrexistingpath").textContent).toBe("No folder selected");
});
it("moves between tabs with arrow keys", () => {
  openNewRepo();
  const create = document.querySelector('[data-project-tab="create"]');
  create.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  expect(document.querySelector('[data-project-tab="existing"]').getAttribute("aria-selected")).toBe("true");
});
it("shows configured-folder lookup failures without opening at an unrelated default", async () => {
  App.call.mockRejectedValue(new Error("Device offline"));
  openNewRepo();
  document.querySelector('[data-project-tab="existing"]').click();
  await flush();
  expect(document.querySelector("#nrerr").textContent).toBe("Device offline");
  expect(openBrowser).not.toHaveBeenCalled();
});
it("does not create on another device after the active device changes", async () => {
  openNewRepo();
  document.querySelector("#nrname").value = "project";
  const original = App.call;
  App.session = { deviceId: "two" };
  App.call = vi.fn();
  document.querySelector("#nrdo").click();
  await flush();
  expect(original).not.toHaveBeenCalled();
  expect(App.call).not.toHaveBeenCalled();
  expect(document.querySelector("#nrerr").textContent).toContain("device changed");
});
it("does not replace a later sheet when a folder lookup resolves after cancellation", async () => {
  let resolve;
  App.call.mockReturnValue(new Promise((done) => { resolve = done; }));
  openNewRepo();
  document.querySelector('[data-project-tab="existing"]').click();
  document.querySelector("#nrcancel").click();
  document.querySelector("#sheet").innerHTML = "Another sheet";
  resolve({ projects_dir: "/projects" });
  await flush();
  expect(openBrowser).not.toHaveBeenCalled();
  expect(document.querySelector("#sheet").textContent).toBe("Another sheet");
});
it("omits a blank optional remote and rejects an empty project name", async () => {
  openNewRepo();
  document.querySelector("#nrdo").click();
  expect(App.call).not.toHaveBeenCalled();
  document.querySelector("#nrname").value = " docs ";
  document.querySelector("#nrremote").value = "   ";
  document.querySelector("#nrdo").click();
  await flush();
  expect(App.call).toHaveBeenCalledWith("project.create", { name: "docs" });
});
it("keeps failed creation editable and does not submit duplicates", async () => {
  let reject;
  App.call.mockReturnValue(new Promise((_, fail) => { reject = fail; }));
  openNewRepo();
  document.querySelector("#nrname").value = "docs";
  document.querySelector("#nrdo").click();
  document.querySelector("#nrform").dispatchEvent(new Event("submit", { cancelable: true }));
  expect(App.call).toHaveBeenCalledTimes(1);
  expect(document.querySelector('[data-project-tab="existing"]').disabled).toBe(true);
  document.querySelector('[data-project-tab="existing"]').click();
  expect(document.querySelector("#nrname")).not.toBeNull();
  reject(new Error("Folder already exists"));
  await flush();
  expect(document.querySelector("#nrname").value).toBe("docs");
  expect(document.querySelector("#nrerr").textContent).toBe("Folder already exists");
  expect(document.querySelector("#nrdo").disabled).toBe(false);
  expect(document.querySelector('[data-project-tab="existing"]').disabled).toBe(false);
});
it("does not dismiss a newer sheet when an earlier create finishes", async () => {
  let resolve;
  App.call.mockReturnValue(new Promise((done) => { resolve = done; }));
  const done = vi.fn();
  openNewRepo(done);
  document.querySelector("#nrname").value = "docs";
  document.querySelector("#nrdo").click();
  document.querySelector("#sheet").innerHTML = "Newer sheet";
  resolve({ project_id: "p1" });
  await flush();
  expect(document.querySelector("#sheet").textContent).toBe("Newer sheet");
  expect(document.querySelector("#scrim").classList.contains("show")).toBe(true);
  expect(done).not.toHaveBeenCalled();
});
