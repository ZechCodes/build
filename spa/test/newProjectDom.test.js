// @vitest-environment jsdom
// The Add-project sheet talks to the machine it was opened with and asks
// nothing about devices: whoever opens it has already resolved which machine
// the project is going on, and names it in the sub line.
import { beforeEach, expect, it, vi } from "vitest";
const { openBrowser } = vi.hoisted(() => ({ openBrowser: vi.fn() }));
vi.mock("../src/sheets/browser.js", () => ({ openBrowser }));
import { openNewRepo } from "../src/sheets/newRepo.js";
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
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
beforeEach(() => {
  vi.resetAllMocks(); document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  callRpc = vi.fn(async (method) => method === "settings.get" ? { projects_dir: "/projects" } : { project_id: "p1" });
});

it("opens the multi-source project form without creation tabs", () => {
  openSheet();
  expect(document.querySelector('[role="tab"]')).toBeNull();
  expect(document.querySelector("legend").textContent).toBe("Workspace folders (optional)");
});

it("names the machine the folders come from", () => {
  openSheet();
  expect(document.querySelector("#sheet .sub").textContent).toContain("Laptop's configured projects folder");
});

it("creates a named empty project in the device projects folder", async () => {
  const done = vi.fn(); openSheet(done);
  document.querySelector("#nrname").value = " docs ";
  document.querySelector("#nrdo").click(); await flush();
  expect(callRpc).toHaveBeenCalledWith("project.create", { name: "docs" });
  expect(done).toHaveBeenCalledWith({ project_id: "p1" });
});

it("creates a name-only project on the currently selected device without loading settings", async () => {
  const done = vi.fn();
  const calls = openSelectableSheet(done, "desk");
  const selector = document.querySelector("#nrdevice");
  selector.value = "lap"; selector.dispatchEvent(new Event("change"));
  document.querySelector("#nrname").value = "docs";
  document.querySelector("#nrdo").click(); await flush();
  expect(calls.desk).not.toHaveBeenCalled();
  expect(calls.lap).toHaveBeenCalledOnce();
  expect(calls.lap).toHaveBeenCalledWith("project.create", { name: "docs" });
  expect(done).toHaveBeenCalledWith({ project_id: "lap-project" }, devices[1]);
});

it("creates one project from mixed local and remote sources", async () => {
  const done = vi.fn(); openSheet(done); document.querySelector("#nrname").value = " platform ";
  document.querySelector("#nraddfolder").click(); await flush(); openBrowser.mock.calls[0][0].onChoose("/projects/api");
  document.querySelector("#nraddremote").click(); const remote = document.querySelector("[data-source-value]");
  remote.value = " https://github.com/acme/web.git "; remote.dispatchEvent(new Event("input"));
  const branch = document.querySelectorAll("[data-source-branch]")[1]; branch.value = " trunk "; branch.dispatchEvent(new Event("input"));
  document.querySelector("#nrdo").click(); await flush();
  expect(callRpc).toHaveBeenCalledWith("project.create", { name: "platform", sources: [
    { path: "/projects/api", name: "api" }, { remote: "https://github.com/acme/web.git", name: "web", base_branch: "trunk" },
  ] });
  expect(done).toHaveBeenCalledWith({ project_id: "p1" });
});

it("generates stable unique mount names and omits a removed source", async () => {
  openSheet(); document.querySelector("#nrname").value = "suite";
  for (let i = 0; i < 2; i += 1) { document.querySelector("#nraddfolder").click(); await flush(); openBrowser.mock.calls[i][0].onChoose(`/where${i}/api`); }
  expect([...document.querySelectorAll("[data-source-name]")].map((node) => node.value)).toEqual(["api", "api-2"]);
  document.querySelector("[data-remove-source]").click(); document.querySelector("#nrdo").click(); await flush();
  expect(callRpc).toHaveBeenCalledWith("project.create", { name: "suite", sources: [{ path: "/where1/api", name: "api-2" }] });
});

it("rejects duplicate mount names and focuses the duplicate", () => {
  openSheet(); document.querySelector("#nrname").value = "suite";
  document.querySelector("#nraddremote").click(); document.querySelector("#nraddremote").click();
  [...document.querySelectorAll("[data-source-value]")].forEach((node, index) => { node.value = `https://host/repo-${index}.git`; node.dispatchEvent(new Event("input")); });
  [...document.querySelectorAll("[data-source-name]")].forEach((node) => { node.value = " API "; node.dispatchEvent(new Event("input")); });
  document.querySelector("#nrdo").click();
  expect(document.querySelector("#nrerr").textContent).toContain("must be unique");
  expect(document.activeElement).toBe(document.querySelectorAll("[data-source-name]")[1]); expect(callRpc).not.toHaveBeenCalled();
});

it("ignores a stale browser choice and enables directory creation in the picker", async () => {
  openSheet(); document.querySelector("#nrname").value = "docs"; document.querySelector("#nraddfolder").click(); await flush();
  expect(openBrowser.mock.calls[0][0].allowCreateDirectory).toBe(true);
  const choose = openBrowser.mock.calls[0][0].onChoose; document.querySelector("#nrback").click(); choose("/stale");
  expect(document.querySelector("#nrsources").textContent).toContain("No folder selected");
});

// The caller's own rpc refuses when its machine goes: the sheet has no device
// question of its own, so it shows whatever that refusal says.
it("shows the message its machine's caller refuses with", async () => {
  openSheet(); document.querySelector("#nrname").value = "docs"; document.querySelector("#nraddremote").click();
  document.querySelector("[data-source-value]").value = "https://example.com/docs.git";
  document.querySelector("[data-source-value]").dispatchEvent(new Event("input"));
  callRpc.mockRejectedValueOnce(new Error("Device offline"));
  document.querySelector("#nrdo").click(); await flush();
  expect(document.querySelector("#nrerr").textContent).toBe("Device offline");
});

it("requires an explicit device when the rail shows all devices", () => {
  openSelectableSheet();
  expect(document.querySelector("#nrdevice").value).toBe("");
  expect(document.querySelector("#nrdevice option").textContent).toBe("Choose a device");
  document.querySelector("#nrname").value = "docs";
  document.querySelector("#nraddremote").click();
  const remote = document.querySelector("[data-source-value]");
  remote.value = "https://example.com/docs.git"; remote.dispatchEvent(new Event("input"));
  document.querySelector("#nrdo").click();
  expect(document.querySelector("#nrerr").textContent).toBe("Choose a device.");
  expect(document.activeElement).toBe(document.querySelector("#nrdevice"));
});

it("switches every local operation to the chosen device and clears only machine-local paths", async () => {
  const done = vi.fn();
  const calls = openSelectableSheet(done, "desk");
  document.querySelector("#nrname").value = "suite";
  document.querySelector("#nraddfolder").click(); await flush();
  expect(calls.desk).toHaveBeenCalledWith("settings.get");
  openBrowser.mock.calls[0][0].onChoose("/desk-projects/api");
  document.querySelector("#nraddremote").click();
  const remote = document.querySelector("[data-source-value]");
  remote.value = "https://example.com/web.git"; remote.dispatchEvent(new Event("input"));

  const selector = document.querySelector("#nrdevice");
  selector.value = "lap"; selector.dispatchEvent(new Event("change"));
  expect(document.querySelector("#nrname").value).toBe("suite");
  expect(document.querySelector("[data-source-value]").value).toBe("https://example.com/web.git");
  expect(document.querySelector("[data-source-row]").textContent).toContain("No folder selected");
  expect(document.querySelector("#nrerr").textContent).toContain("Choose local folders again");

  document.querySelector("[data-choose-source]").click(); await flush();
  expect(calls.lap).toHaveBeenCalledWith("settings.get");
  expect(openBrowser.mock.calls[1][0].startPath).toBe("/lap-projects");
  expect(openBrowser.mock.calls[1][0].callRpc).toBe(calls.lap);
  openBrowser.mock.calls[1][0].onChoose("/lap-projects/api");
  document.querySelector("#nrdo").click(); await flush();
  expect(calls.lap).toHaveBeenCalledWith("project.create", {
    name: "suite",
    sources: [{ path: "/lap-projects/api", name: "api" }, { remote: "https://example.com/web.git", name: "web" }],
  });
  expect(done).toHaveBeenCalledWith({ project_id: "lap-project" }, devices[1]);
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
  document.querySelector("[data-choose-source]").click(); await flush();
  expect(openBrowser.mock.calls[0][0].startPath).toBe("/lap-projects");

  resolveDesk({ projects_dir: "/desk-projects" }); await flush();
  document.querySelector("#nrback").click();
  document.querySelector("[data-choose-source]").click(); await flush();
  expect(lap).toHaveBeenCalledTimes(1);
  expect(openBrowser.mock.calls[1][0].startPath).toBe("/lap-projects");
});

it("disables device selection while project creation is in flight", async () => {
  let finishCreate;
  const lap = vi.fn((method) => method === "project.create"
    ? new Promise((resolve) => { finishCreate = resolve; })
    : Promise.resolve({ projects_dir: "/lap-projects" }));
  openNewRepo(undefined, { devices, defaultDeviceId: "lap", callRpcFor: () => lap });
  document.querySelector("#nrname").value = "docs";
  document.querySelector("#nraddremote").click();
  const remote = document.querySelector("[data-source-value]");
  remote.value = "https://example.com/docs.git"; remote.dispatchEvent(new Event("input"));

  document.querySelector("#nrdo").click();
  expect(document.querySelector("#nrdevice").disabled).toBe(true);
  finishCreate({ project_id: "p1" }); await flush();
});
