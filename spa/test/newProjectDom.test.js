// @vitest-environment jsdom
// The Add-project sheet talks to the machine it was opened with and asks
// nothing about devices: whoever opens it has already resolved which machine
// the project is going on, and names it in the sub line.
import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
const { openBrowser } = vi.hoisted(() => ({ openBrowser: vi.fn() }));
vi.mock("../src/sheets/browser.js", () => ({ openBrowser }));
let openNewRepo;
let writeCached;
let readCached;
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const waitForBrowserCall = (index) => vi.waitFor(() => expect(openBrowser.mock.calls[index]).toBeDefined());
let callRpc;
const openSheet = (onDone) => openNewRepo(onDone, { callRpc, deviceName: "Laptop" });
const devices = [{ id: "desk", name: "Desktop" }, { id: "lap", name: "Laptop" }];
const openSelectableSheet = (onDone, defaultDeviceId) => {
  const calls = {
    desk: vi.fn(async (method) => method === "settings.get" ? { projects_dir: "/desk-projects" } : { project_id: "desk-project" }),
    lap: vi.fn(async (method) => method === "settings.get" ? { projects_dir: "/lap-projects" } : { project_id: "lap-project" }),
  };
  openNewRepo(onDone, { devices, defaultDeviceId, callRpcFor: (id) => calls[id] });
  return calls;
};
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ openNewRepo } = await import("../src/sheets/newRepo.js"));
  ({ writeCached, readCached } = await import("../src/core/localCache.js"));
  vi.resetAllMocks(); document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  callRpc = vi.fn(async (method) => method === "settings.get" ? { projects_dir: "/projects" } : { project_id: "p1" });
});

it("opens the multi-source project form without creation tabs", () => {
  openSheet();
  expect(document.querySelector('[role="tab"]')).toBeNull();
  expect(document.querySelector("legend").textContent).toBe("Workspace folders");
});

it("names the machine the folders come from", () => {
  openSheet();
  expect(document.querySelector("#sheet .sub").textContent).toBe("Add the folders and Git remotes on Laptop that this project works in.");
});

it("restores an unsent project draft from cache without asking a device", async () => {
  const address = { deviceId: "", entityId: "", kind: "ui-draft", sub: "new-project:" };
  await writeCached(address, { name: "cached project", sources: [], selectedDeviceId: "lap" });
  const calls = openSelectableSheet(undefined);
  await vi.waitFor(() => expect(document.querySelector("#nrproject").value).toBe("cached project"));
  expect(document.querySelector("#nrdevice").value).toBe("lap");
  expect(calls.lap).not.toHaveBeenCalled();
});

it("drops another device's local folder when its saved device is gone", async () => {
  const address = { deviceId: "", entityId: "", kind: "ui-draft", sub: "new-project:" };
  await writeCached(address, {
    name: "moved project", selectedDeviceId: "removed-device",
    sources: [{ id: 1, kind: "path", path: "/removed/private", name: "private", base_branch: "", automaticName: false }],
  });
  const calls = openSelectableSheet(undefined, "lap");
  await vi.waitFor(() => expect(document.querySelector("#nrproject")?.value).toBe("moved project"));
  await vi.waitFor(() => expect(document.querySelector("[data-source-row]")?.textContent).toContain("No folder selected"));
  expect(document.querySelector("#nrdevice").value).toBe("lap");
  await vi.waitFor(async () => expect((await readCached(address))?.value.sources[0].path).toBe(""));
  document.querySelector("#nrdo").click();
  expect(calls.lap).not.toHaveBeenCalledWith("project.create", expect.anything());
  expect(document.querySelector("#nrerr").textContent).toContain("Choose");
});

it("keeps the current device when restoring a draft made on another paired device", async () => {
  const address = { deviceId: "", entityId: "", kind: "ui-draft", sub: "new-project:" };
  await writeCached(address, {
    name: "from laptop", selectedDeviceId: "lap",
    sources: [{ id: 1, kind: "path", path: "/lap/private", pathDeviceId: "lap", name: "private", base_branch: "", automaticName: false }],
  });
  const calls = openSelectableSheet(undefined, "desk");
  await vi.waitFor(() => expect(document.querySelector("#nrproject")?.value).toBe("from laptop"));
  expect(document.querySelector("#nrdevice").value).toBe("desk");
  await vi.waitFor(() => expect(document.querySelector("[data-source-row]")?.textContent).toContain("No folder selected"));
  document.querySelector("#nrdo").click();
  expect(calls.desk).not.toHaveBeenCalledWith("project.create", expect.anything());
  expect(calls.lap).not.toHaveBeenCalledWith("project.create", expect.anything());
});

it("writes the project draft after typing and clears it when creation succeeds", async () => {
  const address = { deviceId: "", entityId: "", kind: "ui-draft", sub: "new-project:" };
  const done = vi.fn();
  openSheet(done);
  document.querySelector("#nraddremote").click();
  const remote = document.querySelector("[data-source-value]");
  remote.value = "https://example.com/new.git"; remote.dispatchEvent(new Event("input"));
  const name = document.querySelector("#nrproject");
  name.value = "new project";
  name.dispatchEvent(new Event("input"));
  await vi.waitFor(async () => expect((await readCached(address))?.value.name).toBe("new project"));
  document.querySelector("#nrdo").click();
  await vi.waitFor(() => expect(done).toHaveBeenCalled());
  expect((await readCached(address)).value.name).toBe("");
});

it("opens the folder picker from cached device settings while the settings pull is absent", async () => {
  await writeCached({ deviceId: "lap", entityId: "", kind: "settings" }, { projects_dir: "/cached-projects" });
  const pending = vi.fn(() => new Promise(() => {}));
  openNewRepo(undefined, { devices, defaultDeviceId: "lap", callRpcFor: () => pending });
  document.querySelector("#nraddfolder").click();
  await waitForBrowserCall(0);
  expect(openBrowser.mock.calls[0][0].startPath).toBe("/cached-projects");
  expect(pending).toHaveBeenCalledWith("settings.get");
});

it("opens the picker after a real settings cache announcement while the pull is pending", async () => {
  const pending = vi.fn(() => new Promise(() => {}));
  openNewRepo(undefined, { devices, defaultDeviceId: "lap", callRpcFor: () => pending });
  document.querySelector("#nraddfolder").click();
  await vi.waitFor(() => expect(pending).toHaveBeenCalledWith("settings.get"));
  await writeCached({ deviceId: "lap", entityId: "", kind: "settings" }, { projects_dir: "/announced-projects" });
  await waitForBrowserCall(0);
  expect(openBrowser.mock.calls[0][0].startPath).toBe("/announced-projects");
});

// #185, Zech 13:14Z: "There must always be a folder added, git or not."
it("creates a project under a typed name with its folder", async () => {
  const done = vi.fn(); openSheet(done);
  document.querySelector("#nraddremote").click();
  const remote = document.querySelector("[data-source-value]");
  remote.value = "https://example.com/docs-site.git"; remote.dispatchEvent(new Event("input"));
  document.querySelector("#nrproject").value = " docs ";
  document.querySelector("#nrdo").click(); await flush();
  expect(callRpc).toHaveBeenCalledWith("project.create", { name: "docs", sources: [{ remote: "https://example.com/docs-site.git", name: "docs-site" }] });
  await vi.waitFor(() => expect(done).toHaveBeenCalledWith({ project_id: "p1" }));
});

it("creates a remote-only project on the currently selected device without loading settings", async () => {
  const done = vi.fn();
  const calls = openSelectableSheet(done, "desk");
  const selector = document.querySelector("#nrdevice");
  selector.value = "lap"; selector.dispatchEvent(new Event("change"));
  document.querySelector("#nraddremote").click();
  const remote = document.querySelector("[data-source-value]");
  remote.value = "https://example.com/docs.git"; remote.dispatchEvent(new Event("input"));
  document.querySelector("#nrdo").click(); await flush();
  expect(calls.desk).not.toHaveBeenCalled();
  expect(calls.lap).toHaveBeenCalledOnce();
  expect(calls.lap).toHaveBeenCalledWith("project.create", { name: "docs", sources: [{ remote: "https://example.com/docs.git", name: "docs" }] });
  await vi.waitFor(() => expect(done).toHaveBeenCalledWith({ project_id: "lap-project" }, devices[1]));
});

it("creates one project from mixed local and remote sources", async () => {
  const done = vi.fn(); openSheet(done); document.querySelector("#nrproject").value = " platform ";
  document.querySelector("#nraddfolder").click(); await waitForBrowserCall(0); openBrowser.mock.calls[0][0].onChoose("/projects/api");
  document.querySelector("#nraddremote").click(); const remote = document.querySelector("[data-source-value]");
  remote.value = " https://github.com/acme/web.git "; remote.dispatchEvent(new Event("input"));
  const branch = document.querySelectorAll("[data-source-branch]")[1]; branch.value = " trunk "; branch.dispatchEvent(new Event("input"));
  document.querySelector("#nrdo").click(); await flush();
  expect(callRpc).toHaveBeenCalledWith("project.create", { name: "platform", sources: [
    { path: "/projects/api", name: "api" }, { remote: "https://github.com/acme/web.git", name: "web", base_branch: "trunk" },
  ] });
  await vi.waitFor(() => expect(done).toHaveBeenCalledWith({ project_id: "p1" }));
});

it("generates stable unique mount names and omits a removed source", async () => {
  openSheet(); document.querySelector("#nrproject").value = "suite";
  for (let i = 0; i < 2; i += 1) { document.querySelector("#nraddfolder").click(); await waitForBrowserCall(i); openBrowser.mock.calls[i][0].onChoose(`/where${i}/api`); }
  expect([...document.querySelectorAll("[data-source-name]")].map((node) => node.value)).toEqual(["api", "api-2"]);
  document.querySelector("[data-remove-source]").click(); document.querySelector("#nrdo").click(); await flush();
  expect(callRpc).toHaveBeenCalledWith("project.create", { name: "suite", sources: [{ path: "/where1/api", name: "api-2" }] });
});

it("rejects duplicate mount names and focuses the duplicate", () => {
  openSheet(); document.querySelector("#nrproject").value = "suite";
  document.querySelector("#nraddremote").click(); document.querySelector("#nraddremote").click();
  [...document.querySelectorAll("[data-source-value]")].forEach((node, index) => { node.value = `https://host/repo-${index}.git`; node.dispatchEvent(new Event("input")); });
  [...document.querySelectorAll("[data-source-name]")].forEach((node) => { node.value = " API "; node.dispatchEvent(new Event("input")); });
  document.querySelector("#nrdo").click();
  expect(document.querySelector("#nrerr").textContent).toContain("must be unique");
  expect(document.activeElement).toBe(document.querySelectorAll("[data-source-name]")[1]); expect(callRpc).not.toHaveBeenCalled();
});

it("ignores a stale browser choice and enables directory creation in the picker", async () => {
  openSheet(); document.querySelector("#nrproject").value = "docs"; document.querySelector("#nraddfolder").click(); await waitForBrowserCall(0);
  expect(openBrowser.mock.calls[0][0].allowCreateDirectory).toBe(true);
  expect(openBrowser.mock.calls[0][0].fallbackFromMissingStart).toBe(true);
  const choose = openBrowser.mock.calls[0][0].onChoose; document.querySelector("#nrback").click(); choose("/stale");
  expect(document.querySelector("#nrsources").textContent).toContain("No folder selected");
});

// The caller's own rpc refuses when its machine goes: the sheet has no device
// question of its own, so it shows whatever that refusal says.
it("shows the message its machine's caller refuses with", async () => {
  openSheet(); document.querySelector("#nrproject").value = "docs"; document.querySelector("#nraddremote").click();
  document.querySelector("[data-source-value]").value = "https://example.com/docs.git";
  document.querySelector("[data-source-value]").dispatchEvent(new Event("input"));
  callRpc.mockRejectedValueOnce(new Error("Device offline"));
  document.querySelector("#nrdo").click(); await flush();
  expect(document.querySelector("#nrerr").textContent).toBe("Device offline");
});

// Zech, Sep 27: "show only the device selector then once the device is chosen
// show the rest of the form".
const formParts = ["#nrproject", "#nrsources", "#nraddfolder", "#nraddremote", "#nrdo"];
const partsShown = () => formParts.filter((selector) => document.querySelector(selector));

it("with all devices and none chosen, shows only the device choice and Cancel", () => {
  openSelectableSheet();
  expect(document.querySelector("#nrdevice").value).toBe("");
  expect(document.querySelector("#nrdevice option").textContent).toBe("Choose a device");
  expect(document.querySelector(".sub").textContent).toBe("Choose the device where this project will be created.");
  expect(partsShown()).toEqual([]);
  expect(document.querySelector("#nrcancel")).toBeTruthy();
  expect(document.activeElement).toBe(document.querySelector("#nrdevice"));
  document.querySelector("#nrcancel").click();
  expect(document.querySelector("#scrim").classList.contains("show")).toBe(false);
});

it("choosing a device shows the rest of the form and starts at the folders", () => {
  openSelectableSheet();
  const selector = document.querySelector("#nrdevice");
  selector.value = "lap"; selector.dispatchEvent(new Event("change"));
  expect(partsShown()).toEqual(formParts);
  expect(document.querySelector("#nrdevice").value).toBe("lap");
  expect(document.activeElement).toBe(document.querySelector("#nraddfolder"));
});

it("a restored draft with no device waits on the device choice, and its values survive the reveal", async () => {
  const address = { deviceId: "", entityId: "", kind: "ui-draft", sub: "new-project:" };
  await writeCached(address, {
    name: "Skrift", selectedDeviceId: "",
    sources: [{ id: 1, kind: "remote", path: "", remote: "skrift", name: "skrift", base_branch: "", automaticName: true }],
  });
  openSelectableSheet();
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(partsShown()).toEqual([]);
  const selector = document.querySelector("#nrdevice");
  selector.value = "desk"; selector.dispatchEvent(new Event("change"));
  expect(document.querySelector("#nrproject").value).toBe("Skrift");
  expect(document.querySelector("[data-source-value]").value).toBe("skrift");
  expect(document.activeElement).toBe(document.querySelector("#nraddfolder"));
});

it("a restored draft that names an available device opens on the full form", async () => {
  const address = { deviceId: "", entityId: "", kind: "ui-draft", sub: "new-project:" };
  await writeCached(address, { name: "cached project", sources: [], selectedDeviceId: "lap" });
  openSelectableSheet();
  await vi.waitFor(() => expect(document.querySelector("#nrproject")?.value).toBe("cached project"));
  expect(partsShown()).toEqual(formParts);
  expect(document.querySelector("#nrdevice").value).toBe("lap");
  expect(document.activeElement).toBe(document.querySelector("#nraddfolder"));
});

it("an account with one device opens on the full form", () => {
  openNewRepo(undefined, { devices: [devices[0]], defaultDeviceId: "", callRpcFor: () => vi.fn(async () => ({})) });
  expect(partsShown()).toEqual(formParts);
  expect(document.querySelector("#nrdevice").value).toBe("desk");
  expect(document.activeElement).toBe(document.querySelector("#nraddfolder"));
});

it("a pinned device's sheet is unchanged: the whole form, no device choice", () => {
  openNewRepo(undefined, { callRpc: vi.fn(async () => ({})), deviceName: "Desktop", deviceId: "desk" });
  expect(document.querySelector("#nrdevice")).toBeNull();
  expect(partsShown()).toEqual(formParts);
});

it("switches every local operation to the chosen device and clears only machine-local paths", async () => {
  const done = vi.fn();
  const calls = openSelectableSheet(done, "desk");
  document.querySelector("#nrproject").value = "suite";
  document.querySelector("#nraddfolder").click(); await waitForBrowserCall(0);
  expect(calls.desk).toHaveBeenCalledWith("settings.get");
  openBrowser.mock.calls[0][0].onChoose("/desk-projects/api");
  document.querySelector("#nraddremote").click();
  const remote = document.querySelector("[data-source-value]");
  remote.value = "https://example.com/web.git"; remote.dispatchEvent(new Event("input"));

  const selector = document.querySelector("#nrdevice");
  selector.value = "lap"; selector.dispatchEvent(new Event("change"));
  expect(document.querySelector("#nrproject").value).toBe("suite");
  expect(document.querySelector("[data-source-value]").value).toBe("https://example.com/web.git");
  expect(document.querySelector("[data-source-row]").textContent).toContain("No folder selected");
  expect(document.querySelector("#nrerr").textContent).toContain("Choose local folders again");

  document.querySelector("[data-choose-source]").click(); await waitForBrowserCall(1);
  expect(calls.lap).toHaveBeenCalledWith("settings.get");
  expect(openBrowser.mock.calls[1][0].startPath).toBe("/lap-projects");
  expect(openBrowser.mock.calls[1][0].callRpc).toBe(calls.lap);
  openBrowser.mock.calls[1][0].onChoose("/lap-projects/api");
  document.querySelector("#nrdo").click(); await flush();
  expect(calls.lap).toHaveBeenCalledWith("project.create", {
    name: "suite",
    sources: [{ path: "/lap-projects/api", name: "api" }, { remote: "https://example.com/web.git", name: "web" }],
  });
  await vi.waitFor(() => expect(done).toHaveBeenCalledWith({ project_id: "lap-project" }, devices[1]));
});

it("never caches a stale projects folder after returning and switching devices", async () => {
  let resolveDesk;
  const desk = vi.fn(() => new Promise((resolve) => { resolveDesk = resolve; }));
  const lap = vi.fn(async () => ({ projects_dir: "/lap-projects" }));
  openNewRepo(undefined, { devices, defaultDeviceId: "desk", callRpcFor: (id) => id === "desk" ? desk : lap });

  document.querySelector("#nraddfolder").click();
  document.querySelector("#nrback").click();
  const selector = document.querySelector("#nrdevice");
  selector.value = "lap"; selector.dispatchEvent(new Event("change"));
  document.querySelector("[data-choose-source]").click(); await waitForBrowserCall(0);
  expect(openBrowser.mock.calls[0][0].startPath).toBe("/lap-projects");

  resolveDesk({ projects_dir: "/desk-projects" }); await flush();
  document.querySelector("#nrback").click();
  document.querySelector("[data-choose-source]").click(); await waitForBrowserCall(1);
  expect(lap).toHaveBeenCalledTimes(1);
  expect(openBrowser.mock.calls[1][0].startPath).toBe("/lap-projects");
});

it("disables device selection while project creation is in flight", async () => {
  let finishCreate;
  const lap = vi.fn((method) => method === "project.create"
    ? new Promise((resolve) => { finishCreate = resolve; })
    : Promise.resolve({ projects_dir: "/lap-projects" }));
  openNewRepo(undefined, { devices, defaultDeviceId: "lap", callRpcFor: () => lap });
  document.querySelector("#nrproject").value = "docs";
  document.querySelector("#nraddremote").click();
  const remote = document.querySelector("[data-source-value]");
  remote.value = "https://example.com/docs.git"; remote.dispatchEvent(new Event("input"));

  document.querySelector("#nrdo").click();
  expect(document.querySelector("#nrdevice").disabled).toBe(true);
  finishCreate({ project_id: "p1" }); await flush();
});

// #185, Zech 13:11Z Sep 27: "Awkward optional but required… the flow needs to
// be pick device, add folders, name project if you want. Otherwise the name
// should autofill based on the name/repo of the first directory added."
const nameField = () => document.querySelector("#nrproject");
const addRemoteRow = (value) => {
  document.querySelector("#nraddremote").click();
  const remote = [...document.querySelectorAll("[data-source-value]")].at(-1);
  remote.value = value; remote.dispatchEvent(new Event("input"));
  return remote;
};
const typeName = (value) => { nameField().value = value; nameField().dispatchEvent(new Event("input")); };
const submitSheet = () => document.querySelector("#nrdo").click();

it("orders the sheet device, folders, name, then Cancel and Create", () => {
  openSelectableSheet(undefined, "desk");
  const order = ["#nrdevice", "#nraddfolder", "#nraddremote", "#nrproject", "#nrcancel", "#nrdo"].map((selector) => document.querySelector(selector));
  const positions = order.map((node) => [...document.querySelectorAll("#sheet *")].indexOf(node));
  expect(positions.every((position, index) => index === 0 || position > positions[index - 1])).toBe(true);
  expect(document.querySelector('label[for="nrproject"]').textContent).toBe("Project name (optional)");
  expect(nameField().hasAttribute("required")).toBe(false);
  document.querySelector("#nrcancel").click();
  openSheet(); // the pinned sheet: the same order, without a device
  const pinned = ["#nraddfolder", "#nrproject", "#nrdo"].map((selector) => [...document.querySelectorAll("#sheet *")].indexOf(document.querySelector(selector)));
  expect(pinned[0]).toBeLessThan(pinned[1]);
  expect(pinned[1]).toBeLessThan(pinned[2]);
  expect(document.activeElement).toBe(document.querySelector("#nraddfolder"));
});

it("with a remote and no typed name, the name follows the remote's repository", async () => {
  const calls = openSelectableSheet(undefined, "desk");
  addRemoteRow("git@github.com:ZechCodes/Skrift.git");
  expect(nameField().value).toBe("Skrift");
  submitSheet();
  await vi.waitFor(() => expect(calls.desk).toHaveBeenCalledWith("project.create", { name: "Skrift", sources: [{ remote: "git@github.com:ZechCodes/Skrift.git", name: "Skrift" }] }));
});

it("with a chosen folder and no typed name, the name follows the folder", async () => {
  openSelectableSheet(undefined, "desk");
  document.querySelector("#nraddfolder").click();
  await waitForBrowserCall(0);
  openBrowser.mock.calls[0][0].onChoose("/desk-projects/api");
  expect(nameField().value).toBe("api");
  expect(nameField().placeholder).toBe("api");
});

it("a typed name stops following the folders, and clearing it follows again", () => {
  openSelectableSheet(undefined, "desk");
  const remote = addRemoteRow("git@github.com:ZechCodes/Skrift.git");
  typeName("My project");
  remote.value = "git@github.com:ZechCodes/other.git"; remote.dispatchEvent(new Event("input"));
  expect(nameField().value).toBe("My project");
  typeName("");
  expect(nameField().placeholder).toBe("other");
  remote.value = "git@github.com:ZechCodes/third.git"; remote.dispatchEvent(new Event("input"));
  expect(nameField().value).toBe("third");
});

it("removing the first folder derives the name from the one now first", () => {
  openSelectableSheet(undefined, "desk");
  addRemoteRow("git@github.com:ZechCodes/Skrift.git");
  addRemoteRow("git@github.com:ZechCodes/build-web.git");
  expect(nameField().value).toBe("Skrift");
  document.querySelector("[data-remove-source]").click();
  expect(nameField().value).toBe("build-web");
});

it("with no folder or remote, even with a name, says so in the sheet and starts at Add folder", () => {
  const calls = openSelectableSheet(undefined, "desk");
  typeName("docs");
  submitSheet();
  expect(document.querySelector("#nrerr").textContent).toBe("Add a folder or Git remote.");
  expect(document.activeElement).toBe(document.querySelector("#nraddfolder"));
  expect(document.querySelector("#nrform").noValidate).toBe(true);
  expect(calls.desk).not.toHaveBeenCalledWith("project.create", expect.anything());
});

it("a draft keeps whether its name was typed or followed a folder", async () => {
  const address = { deviceId: "", entityId: "", kind: "ui-draft", sub: "new-project:" };
  const remote = { id: 1, kind: "remote", path: "", remote: "git@github.com:ZechCodes/Skrift.git", name: "Skrift", base_branch: "", automaticName: true };
  openSelectableSheet(undefined, "desk");
  addRemoteRow("git@github.com:ZechCodes/Skrift.git");
  await vi.waitFor(async () => expect((await readCached(address))?.value).toMatchObject({ name: "Skrift", nameAutomatic: true }));
  typeName("Mine");
  await vi.waitFor(async () => expect((await readCached(address))?.value).toMatchObject({ name: "Mine", nameAutomatic: false }));

  document.querySelector("#nrcancel").click();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  await writeCached(address, { name: "Skrift", nameAutomatic: true, sources: [remote], selectedDeviceId: "desk" });
  openSelectableSheet(undefined, "desk");
  await vi.waitFor(() => expect(nameField()?.value).toBe("Skrift"));
  const row = document.querySelector("[data-source-value]");
  row.value = "git@github.com:ZechCodes/renamed.git"; row.dispatchEvent(new Event("input"));
  expect(nameField().value).toBe("renamed"); // still following: it was never typed
  // Let that edit's draft write land before the next opening's draft is set.
  await vi.waitFor(async () => expect((await readCached(address))?.value).toMatchObject({ name: "renamed", nameAutomatic: true }));

  document.querySelector("#nrcancel").click();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  await writeCached(address, { name: "Mine", nameAutomatic: false, sources: [remote], selectedDeviceId: "desk" });
  openSelectableSheet(undefined, "desk");
  await vi.waitFor(() => expect(nameField()?.value).toBe("Mine"));
  const again = document.querySelector("[data-source-value]");
  again.value = "git@github.com:ZechCodes/renamed.git"; again.dispatchEvent(new Event("input"));
  expect(nameField().value).toBe("Mine"); // typed: it stays
});

// Review of 2667723b: a name typed to equal the folder's must still be the
// reader's, and what project.create sends is the proof.
const createCall = (calls) => calls.desk.mock.calls.find(([method]) => method === "project.create")?.[1];

it("a typed name equal to the derived one is the reader's: a later remote change does not replace it", async () => {
  const calls = openSelectableSheet(undefined, "desk");
  const remote = addRemoteRow("git@github.com:ZechCodes/Skrift.git");
  typeName("Custom");
  typeName("");
  typeName("Skrift");
  remote.value = "git@github.com:ZechCodes/other.git"; remote.dispatchEvent(new Event("input"));
  expect(nameField().value).toBe("Skrift");
  submitSheet();
  await vi.waitFor(() => expect(createCall(calls)).toEqual({ name: "Skrift", sources: [{ remote: "git@github.com:ZechCodes/other.git", name: "other" }] }));
});

it("a cleared name creates under the first folder's name", async () => {
  const calls = openSelectableSheet(undefined, "desk");
  addRemoteRow("git@github.com:ZechCodes/Skrift.git");
  typeName("Custom");
  typeName("");
  submitSheet();
  await vi.waitFor(() => expect(createCall(calls)).toEqual({ name: "Skrift", sources: [{ remote: "git@github.com:ZechCodes/Skrift.git", name: "Skrift" }] }));
});

it("removing the first folder creates under the next one's name", async () => {
  const calls = openSelectableSheet(undefined, "desk");
  addRemoteRow("git@github.com:ZechCodes/Skrift.git");
  addRemoteRow("git@github.com:ZechCodes/build-web.git");
  document.querySelector("[data-remove-source]").click();
  submitSheet();
  await vi.waitFor(() => expect(createCall(calls)).toEqual({ name: "build-web", sources: [{ remote: "git@github.com:ZechCodes/build-web.git", name: "build-web" }] }));
});

it("a draft from before nameAutomatic keeps the name it holds as typed", async () => {
  const address = { deviceId: "", entityId: "", kind: "ui-draft", sub: "new-project:" };
  await writeCached(address, {
    name: "Legacy", selectedDeviceId: "desk",
    sources: [{ id: 1, kind: "remote", path: "", remote: "git@github.com:ZechCodes/Skrift.git", name: "Skrift", base_branch: "", automaticName: true }],
  });
  const calls = openSelectableSheet(undefined, "desk");
  await vi.waitFor(() => expect(nameField()?.value).toBe("Legacy"));
  const row = document.querySelector("[data-source-value]");
  row.value = "git@github.com:ZechCodes/other.git"; row.dispatchEvent(new Event("input"));
  submitSheet();
  await vi.waitFor(() => expect(createCall(calls)).toEqual({ name: "Legacy", sources: [{ remote: "git@github.com:ZechCodes/other.git", name: "other" }] }));
});
