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
beforeEach(() => {
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
  expect(document.querySelector("#sheet .sub").textContent).toContain("from Laptop");
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
