// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { openBrowser } from "../src/sheets/browser.js";
const listing = (path) => ({ path, parent: "/", is_git: false, entries: [] });
beforeEach(() => {
  vi.clearAllMocks();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
});
it("uses the supplied device connection and initial folder", async () => {
  const callRpc = vi.fn().mockResolvedValue(listing("/device-projects"));
  const onChoose = vi.fn();
  await openBrowser({ title: "Projects", callRpc, startPath: "/device-projects", onChoose });
  expect(document.querySelector("#sheet > .settings-sheet-frame > .settings-sheet-header h3").textContent).toBe("Projects");
  expect(document.querySelector(".settings-sheet-body .browse-list")).not.toBeNull();
  expect(callRpc).toHaveBeenCalledWith("fs.list", { path: "/device-projects" });
  document.querySelector("#choosecur").click();
  expect(onChoose).toHaveBeenCalledWith("/device-projects");
});
it("can render inside a host without replacing its surrounding controls", async () => {
  document.querySelector("#sheet").innerHTML = '<div id="tabs">tabs</div><div id="browser-host"></div><button id="outside">Add project</button>';
  const host = document.querySelector("#browser-host");
  const callRpc = vi.fn().mockResolvedValue(listing("/device-projects"));
  await openBrowser({ title: "Projects", callRpc, startPath: "/device-projects", container: host, onChoose: vi.fn() });
  expect(document.querySelector("#tabs").textContent).toBe("tabs");
  expect(document.querySelector("#outside")).not.toBeNull();
  expect(host.querySelector("#choosecur")).not.toBeNull();
  expect(host.querySelector("#bcancel")).toBeNull();
  expect(host.querySelector(".settings-sheet-frame")).toBeNull();
});
it("creates a directory in the current folder and opens it", async () => {
  const callRpc = vi.fn()
    .mockResolvedValueOnce(listing("/projects"))
    .mockResolvedValueOnce({ path: "/projects/new source" })
    .mockResolvedValueOnce(listing("/projects/new source"));
  await openBrowser({ title: "Projects", callRpc, allowCreateDirectory: true, onChoose: vi.fn() });
  document.querySelector("#bdirname").value = " new source ";
  document.querySelector("#bmkdir").click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(callRpc).toHaveBeenNthCalledWith(2, "fs.mkdir", { parent: "/projects", name: "new source" });
  expect(callRpc).toHaveBeenNthCalledWith(3, "fs.list", { path: "/projects/new source" });
  expect(document.querySelector(".browse-path").textContent).toBe("/projects/new source");
});
it("validates directory names before calling the device", async () => {
  const callRpc = vi.fn().mockResolvedValue(listing("/projects"));
  await openBrowser({ title: "Projects", callRpc, allowCreateDirectory: true, onChoose: vi.fn() });
  document.querySelector("#bdirname").value = "../escape";
  document.querySelector("#bmkdir").click();
  expect(document.querySelector("#berr").textContent).toContain("single folder name");
  expect(callRpc).toHaveBeenCalledTimes(1);
  expect(document.activeElement).toBe(document.querySelector("#bdirname"));
});
it("ignores a directory created after the embedded picker is replaced", async () => {
  document.querySelector("#sheet").innerHTML = '<div id="browser-host"></div>';
  const host = document.querySelector("#browser-host");
  let finishCreate;
  const callRpc = vi.fn()
    .mockResolvedValueOnce(listing("/projects"))
    .mockImplementationOnce(() => new Promise((resolve) => { finishCreate = resolve; }));
  await openBrowser({ title: "Projects", callRpc, allowCreateDirectory: true, container: host, onChoose: vi.fn() });
  host.querySelector("#bdirname").value = "later";
  host.querySelector("#bmkdir").click();
  host.innerHTML = "Project form restored";
  finishCreate({ path: "/projects/later" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(host.textContent).toBe("Project form restored");
  expect(callRpc).toHaveBeenCalledTimes(2);
});
it("does not overwrite newer embedded content when a listing completes late", async () => {
  document.querySelector("#sheet").innerHTML = '<div id="browser-host"></div>';
  const host = document.querySelector("#browser-host");
  let resolve;
  const callRpc = vi.fn(() => new Promise((done) => { resolve = done; }));
  const opening = openBrowser({ title: "Projects", callRpc, container: host, onChoose: vi.fn() });
  host.innerHTML = "New tab content";
  resolve(listing("/projects"));
  await opening;
  expect(host.textContent).toBe("New tab content");
});
it("shows initial failures and keeps Cancel available", async () => {
  const callRpc = vi.fn().mockRejectedValue(new Error("Device offline"));
  await openBrowser({ title: "Projects", callRpc });
  expect(document.querySelector("#berr").textContent).toBe("Device offline");
  document.querySelector("#bcancel").click();
  expect(document.querySelector("#scrim").classList.contains("show")).toBe(false);
});
it("falls back to the device home only when the configured initial folder is missing", async () => {
  const callRpc = vi.fn()
    .mockRejectedValueOnce(new Error("cannot open /gone/projects: No such file or directory (os error 2)"))
    .mockResolvedValueOnce(listing("/home/me"));
  await openBrowser({ title: "Projects", callRpc, startPath: "/gone/projects", fallbackFromMissingStart: true, onChoose: vi.fn() });
  expect(callRpc).toHaveBeenNthCalledWith(1, "fs.list", { path: "/gone/projects" });
  expect(callRpc).toHaveBeenNthCalledWith(2, "fs.list", {});
  expect(document.querySelector(".browse-path").textContent).toBe("/home/me");
});
it("does not fall back from an initial permission failure", async () => {
  const callRpc = vi.fn().mockRejectedValue(new Error("cannot open /restricted: Permission denied (os error 13)"));
  await openBrowser({ title: "Projects", callRpc, startPath: "/restricted", fallbackFromMissingStart: true });
  expect(callRpc).toHaveBeenCalledOnce();
  expect(document.querySelector("#berr").textContent).toContain("Permission denied");
});
it("does not fall back when later navigation reaches a missing folder", async () => {
  const callRpc = vi.fn()
    .mockResolvedValueOnce({ ...listing("/projects"), entries: [{ name: "Gone", path: "/projects/gone", is_git: false, is_hidden: false }] })
    .mockRejectedValueOnce(new Error("cannot open /projects/gone: No such file or directory (os error 2)"));
  await openBrowser({ title: "Projects", callRpc, startPath: "/projects", fallbackFromMissingStart: true });
  document.querySelectorAll(".browse-nav")[1].click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(callRpc).toHaveBeenCalledTimes(2);
  expect(document.querySelector("#berr").textContent).toContain("No such file or directory");
});
it("does not request home after a missing initial folder finishes behind cancellation", async () => {
  let rejectInitial;
  const callRpc = vi.fn(() => new Promise((resolve, reject) => { rejectInitial = reject; }));
  const opening = openBrowser({ title: "Projects", callRpc, startPath: "/gone", fallbackFromMissingStart: true });
  document.querySelector("#bcancel").click();
  rejectInitial(new Error("cannot open /gone: No such file or directory (os error 2)"));
  await opening;
  expect(callRpc).toHaveBeenCalledOnce();
});
it("surfaces a failed home fallback without retrying again", async () => {
  const callRpc = vi.fn()
    .mockRejectedValueOnce(new Error("cannot open /gone: No such file or directory (os error 2)"))
    .mockRejectedValueOnce(new Error("cannot open home: Permission denied (os error 13)"));
  await openBrowser({ title: "Projects", callRpc, startPath: "/gone", fallbackFromMissingStart: true });
  expect(callRpc).toHaveBeenCalledTimes(2);
  expect(document.querySelector("#berr").textContent).toContain("Permission denied");
});
it("does not paint a home fallback that completes after its embedded host is replaced", async () => {
  document.querySelector("#sheet").innerHTML = '<div id="browser-host"></div>';
  const host = document.querySelector("#browser-host");
  let resolveHome;
  const callRpc = vi.fn()
    .mockRejectedValueOnce(new Error("cannot open /gone: No such file or directory (os error 2)"))
    .mockImplementationOnce(() => new Promise((resolve) => { resolveHome = resolve; }));
  const opening = openBrowser({ title: "Projects", callRpc, startPath: "/gone", fallbackFromMissingStart: true, container: host });
  await new Promise((resolve) => setTimeout(resolve, 0));
  host.innerHTML = "New content";
  resolveHome(listing("/home/me"));
  await opening;
  expect(host.textContent).toBe("New content");
});
it("does not overwrite another sheet when a listing completes late", async () => {
  let resolve;
  const callRpc = vi.fn(() => new Promise((done) => { resolve = done; }));
  const opening = openBrowser({ title: "Projects", callRpc });
  document.querySelector("#sheet").innerHTML = "Another sheet";
  resolve(listing("/projects"));
  await opening;
  expect(document.querySelector("#sheet").textContent).toBe("Another sheet");
});
it("does not paint a listing after cancelling loading", async () => {
  let resolve;
  const callRpc = vi.fn(() => new Promise((done) => { resolve = done; }));
  const opening = openBrowser({ title: "Projects", callRpc });
  document.querySelector("#bcancel").click();
  resolve(listing("/projects"));
  await opening;
  expect(document.querySelector("#choosecur")).toBeNull();
});
it("provides native navigation controls for folders and the parent", async () => {
  const callRpc = vi.fn().mockResolvedValue({ ...listing("/projects"), entries: [{ name: "Nested", path: "/projects/nested", is_git: false, is_hidden: false }] });
  await openBrowser({ title: "Projects", callRpc });
  const navigation = [...document.querySelectorAll(".browse-nav")];
  expect(navigation.map((button) => button.tagName)).toEqual(["BUTTON", "BUTTON"]);
  expect(navigation.map((button) => button.dataset.path)).toEqual(["/", "/projects/nested"]);
  navigation[1].click();
  expect(callRpc).toHaveBeenLastCalledWith("fs.list", { path: "/projects/nested" });
});
