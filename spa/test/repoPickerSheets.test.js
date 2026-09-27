// @vitest-environment jsdom
// The other two Git remote inputs — a project's "Clone url" and a workspace's
// — search the same cached list, and each sheet asks its machine once when it
// opens. Unmocked, like the New project sheet's own tests (repoPickerDom).
import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let openProjectSettings, openWorkspaceSettings, writeCached, greetBridge, githubReposAddress, projectSettingsAddress, workspaceSettingsAddress;

const REPOS = [
  { name_with_owner: "zech/build", ssh_url: "git@github.com:zech/build.git", url: "https://github.com/zech/build", private: true, pushed_at: "2026-09-24T22:00:00Z" },
  { name_with_owner: "smarter-dev/bot", ssh_url: "git@github.com:smarter-dev/bot.git", url: "https://github.com/smarter-dev/bot", private: false, pushed_at: "2026-09-20T08:00:00Z" },
];
const call = () => vi.fn((method) => (method === "github.repos" ? Promise.resolve({ repos: REPOS }) : new Promise(() => {})));
const options = () => [...document.querySelectorAll('[role="option"] .repo-picker-name')].map((name) => name.textContent);
const pickFirst = (input, query) => {
  input.focus();
  input.value = query;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ openProjectSettings } = await import("../src/sheets/projectSettings.js"));
  ({ openWorkspaceSettings } = await import("../src/sheets/workspaceSettings.js"));
  ({ writeCached } = await import("../src/core/localCache.js"));
  ({ greetBridge } = await import("../src/core/changeEvents.js"));
  ({ githubReposAddress } = await import("../src/core/githubRepos.js"));
  ({ projectSettingsAddress, workspaceSettingsAddress } = await import("../src/core/settingsRecords.js"));
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  await greetBridge(async () => ({ api_version: "1.22.0", capabilities: ["github.repos"] }), { deviceId: "dev-1" });
});

it("a project's Clone url searches the machine's repositories", async () => {
  await writeCached(projectSettingsAddress("dev-1", "proj-1"), { project_id: "proj-1", name: "build", path: "/p/build", base_branch: "main", sources: [] });
  const callRpc = call();
  openProjectSettings("proj-1", { callRpc, deviceId: "dev-1" });
  expect(callRpc).toHaveBeenCalledWith("github.repos", {}, { timeoutMs: 30_000 });
  await vi.waitFor(() => expect(document.querySelector("#psaddremote")).toBeTruthy());
  document.querySelector("#psaddremote").click();
  const input = document.querySelector("#psremoteurl");
  input.focus();
  input.value = "bot";
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await vi.waitFor(() => expect(options()).toEqual(["smarter-dev/bot"]));
  pickFirst(input, "bot");
  expect(input.value).toBe("git@github.com:smarter-dev/bot.git");
});

it("a workspace's Clone url searches the machine's repositories", async () => {
  await writeCached(githubReposAddress("dev-1"), { repos: REPOS, refusal: "" });
  await writeCached(workspaceSettingsAddress("dev-1", "ws-1"), { id: "ws-1", project_id: "proj-1", directories: [] });
  await writeCached(projectSettingsAddress("dev-1", "proj-1"), { project_id: "proj-1", sources: [] });
  const callRpc = call();
  openWorkspaceSettings({ id: "ws-1", name: "work", workspaceKey: "dev-1/ws-1" }, { callRpc, catalog: new Promise(() => {}), storage: localStorage });
  expect(callRpc).toHaveBeenCalledWith("github.repos", {}, { timeoutMs: 30_000 });
  await vi.waitFor(() => expect(document.querySelector("#wsdiradd")).toBeTruthy());
  const choice = document.querySelector("#wsdiradd");
  choice.value = "remote";
  choice.dispatchEvent(new Event("change"));
  await vi.waitFor(() => {
    const input = document.querySelector("#wsdirremote");
    input.focus();
    input.value = "zb";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(options()).toEqual(["zech/build"]);
  });
  const input = document.querySelector("#wsdirremote");
  pickFirst(input, "zb");
  expect(input.value).toBe("git@github.com:zech/build.git");
});

// Review of 374ffa3f, finding 1: a settings sheet that waited for its machine
// to greet kept waiting after it closed, so the next sheet open on screen made
// it "wanted" again and both asked — the closed one's answer landing last.
it("a closed settings sheet stops waiting: only the sheet on screen asks when the machine greets", async () => {
  vi.resetModules();
  ({ openProjectSettings } = await import("../src/sheets/projectSettings.js"));
  ({ writeCached } = await import("../src/core/localCache.js"));
  ({ greetBridge } = await import("../src/core/changeEvents.js"));
  ({ projectSettingsAddress } = await import("../src/core/settingsRecords.js"));
  await writeCached(projectSettingsAddress("dev-1", "proj-1"), { project_id: "proj-1", name: "build", path: "/p/build", base_branch: "main", sources: [] });
  const first = call();
  openProjectSettings("proj-1", { callRpc: first, deviceId: "dev-1" }); // not greeted yet: nothing asked
  await vi.waitFor(() => expect(document.querySelector("#pscancel")).toBeTruthy());
  document.querySelector("#pscancel").click();
  const second = call();
  openProjectSettings("proj-1", { callRpc: second, deviceId: "dev-1" });
  await greetBridge(async () => ({ api_version: "1.22.0", capabilities: ["github.repos"] }), { deviceId: "dev-1" });
  await vi.waitFor(() => expect(second.mock.calls.filter(([method]) => method === "github.repos")).toHaveLength(1));
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(first.mock.calls.filter(([method]) => method === "github.repos")).toHaveLength(0);
});
