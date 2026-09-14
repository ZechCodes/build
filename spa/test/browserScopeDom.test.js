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
