// @vitest-environment jsdom
// The Git remote URL on the New project sheet searches the machine's GitHub
// repositories: the list is the device's cached github.repos answer, painted
// from the cache, and opening the sheet asks the machine once in the
// background. Nothing here is mocked: the sheet, the cache, the capability
// read off a real greeting, and the picker are the modules the app runs.
import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let ApiError, openNewRepo, writeCached, readCached, greetBridge, githubReposAddress, startGithubRepos, uiAddress, DEVICES_ADDRESS;

const REPOS = [
  { name_with_owner: "zech/build", description: "Agentic IDE", ssh_url: "git@github.com:zech/build.git", url: "https://github.com/zech/build", private: true, pushed_at: "2026-09-24T22:00:00Z" },
  { name_with_owner: "smarter-dev/bot", description: "Discord bot", ssh_url: "git@github.com:smarter-dev/bot.git", url: "https://github.com/smarter-dev/bot", private: false, pushed_at: "2026-09-20T08:00:00Z" },
  { name_with_owner: "owner/repo", ssh_url: "git@github.com:owner/repo.git", url: "https://github.com/owner/repo", private: false, pushed_at: "2026-09-01T00:00:00Z" },
];
const REFUSAL = "Build cannot list GitHub repositories on desk because gh is not signed in. Run `gh auth login` on desk.";

const greet = (capabilities) => greetBridge(async () => ({ api_version: "1.22.0", capabilities }), { deviceId: "desk" });

/** How the desk's bridge refuses github.repos on the wire, through the v1
 *  adapter: the sentence, with the code the bridge sent. */
const bridgeRefusal = (message) => new ApiError("unavailable", message);

/** The desk's caller: answers github.repos with `repos`, or refuses it. */
const deskCall = ({ repos = REPOS, refuse = null } = {}) => vi.fn(async (method) => {
  if (method === "github.repos") {
    if (refuse) throw bridgeRefusal(refuse);
    return { repos };
  }
  if (method === "settings.get") return { projects_dir: "/desk-projects" };
  return { project_id: "desk-project" };
});

const open = (call, { defaultDeviceId = "desk" } = {}) => openNewRepo(vi.fn(), { devices: [{ id: "desk", name: "Desktop" }], defaultDeviceId, callRpcFor: () => call });
const reposCalls = (call) => call.mock.calls.filter(([method]) => method === "github.repos");
const addRemote = () => {
  document.querySelector("#nraddremote").click();
  return [...document.querySelectorAll("[data-source-value]")].at(-1);
};
const type = (input, value) => {
  input.focus();
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
};
const key = (input, name) => {
  const event = new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true });
  input.dispatchEvent(event);
  return event;
};
const options = () => [...document.querySelectorAll('[role="option"]')].map((option) => option.querySelector(".repo-picker-name").textContent);
const listbox = () => document.querySelector('[role="listbox"]');
const cacheList = (value) => writeCached(githubReposAddress("desk"), value);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ openNewRepo } = await import("../src/sheets/newRepo.js"));
  ({ ApiError } = await import("../src/core/bridgeApi/v1/index.js"));
  ({ writeCached, readCached, DEVICES_ADDRESS } = await import("../src/core/localCache.js"));
  ({ uiAddress } = await import("../src/core/localUiState.js"));
  ({ greetBridge } = await import("../src/core/changeEvents.js"));
  ({ githubReposAddress, startGithubRepos } = await import("../src/core/githubRepos.js"));
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
});

it("opening the sheet asks the machine once and the picker reads the list it cached", async () => {
  await greet(["github.repos"]);
  const call = deskCall();
  open(call);
  await vi.waitFor(async () => expect((await readCached(githubReposAddress("desk")))?.value?.repos).toEqual(REPOS));
  addRemote();
  const input = addRemote();
  expect(call.mock.calls.filter(([method]) => method === "github.repos")).toHaveLength(1);
  type(input, "zb");
  await vi.waitFor(() => expect(options()).toEqual(["zech/build"]));
  expect(input.getAttribute("role")).toBe("combobox");
  expect(input.getAttribute("aria-expanded")).toBe("true");
  expect(document.querySelector(".repo-picker-lock")).not.toBeNull();
  expect(document.querySelector('[role="option"]').textContent).toContain("Agentic IDE");
});

it("filters the cached list as the user types, contiguous hits first", async () => {
  await greet(["github.repos"]);
  await cacheList({ repos: REPOS, refusal: "" });
  open(deskCall());
  const input = addRemote();
  type(input, "own/rep");
  await vi.waitFor(() => expect(options()[0]).toBe("owner/repo"));
  type(input, "bot");
  await vi.waitFor(() => expect(options()).toEqual(["smarter-dev/bot"]));
});

it("a keyboard pick fills the SSH URL and the folder label, and does not submit", async () => {
  await greet(["github.repos"]);
  await cacheList({ repos: REPOS, refusal: "" });
  const call = deskCall();
  open(call);
  const input = addRemote();
  type(input, "zech/b");
  await vi.waitFor(() => expect(options()).toEqual(["zech/build"]));
  key(input, "ArrowDown");
  expect(input.getAttribute("aria-activedescendant")).toBe(document.querySelector('[role="option"]').id);
  expect(key(input, "Enter").defaultPrevented).toBe(true);
  expect(input.value).toBe("git@github.com:zech/build.git");
  expect(document.querySelector("[data-source-name]").value).toBe("build");
  expect(listbox().hidden).toBe(true);
  expect(call).not.toHaveBeenCalledWith("project.create", expect.anything());
});

it("a click on a row picks it", async () => {
  await greet(["github.repos"]);
  await cacheList({ repos: REPOS, refusal: "" });
  open(deskCall());
  const input = addRemote();
  type(input, "bot");
  await vi.waitFor(() => expect(options()).toEqual(["smarter-dev/bot"]));
  document.querySelector('[role="option"]').click();
  expect(input.value).toBe("git@github.com:smarter-dev/bot.git");
  expect(document.querySelector("[data-source-name]").value).toBe("bot");
});

it("a typed URL that matches nothing is sent unchanged", async () => {
  await greet(["github.repos"]);
  await cacheList({ repos: REPOS, refusal: "" });
  const call = deskCall();
  open(call);
  document.querySelector("#nrproject").value = "typed";
  document.querySelector("#nrproject").dispatchEvent(new Event("input", { bubbles: true }));
  const input = addRemote();
  type(input, "git@example.com:someone/else.git");
  expect(options()).toEqual([]);
  expect(key(input, "Enter").defaultPrevented).toBe(false);
  document.querySelector("#nrdo").click();
  await vi.waitFor(() => expect(call).toHaveBeenCalledWith("project.create", {
    name: "typed", sources: [{ remote: "git@example.com:someone/else.git", name: "else" }],
  }));
});

it("Escape closes the list and leaves the sheet open", async () => {
  await greet(["github.repos"]);
  await cacheList({ repos: REPOS, refusal: "" });
  open(deskCall());
  const input = addRemote();
  type(input, "b");
  await vi.waitFor(() => expect(listbox().hidden).toBe(false));
  const escape = key(input, "Escape");
  expect(escape.defaultPrevented).toBe(true);
  expect(listbox().hidden).toBe(true);
  expect(document.querySelector("#scrim").classList.contains("show")).toBe(true);
});

it("with no cached list and a refusal, says the bridge's sentence once and leaves a plain field", async () => {
  await greet(["github.repos"]);
  open(deskCall({ refuse: REFUSAL }));
  addRemote();
  addRemote();
  const first = document.querySelector("[data-source-value]");
  await vi.waitFor(() => expect(document.querySelector(".repo-picker-note:not([hidden])")?.textContent).toBe(REFUSAL));
  expect([...document.querySelectorAll(".repo-picker-note:not([hidden])")]).toHaveLength(1);
  expect(first.hasAttribute("role")).toBe(false);
  type(first, "zech");
  expect(options()).toEqual([]);
});

it("a stale cached list still filters when the refresh is refused, and says nothing", async () => {
  await greet(["github.repos"]);
  await cacheList({ repos: REPOS, refusal: "" });
  const call = deskCall({ refuse: REFUSAL });
  open(call);
  await vi.waitFor(async () => expect((await readCached(githubReposAddress("desk")))?.value?.refusal).toBe(REFUSAL));
  const input = addRemote();
  type(input, "bot");
  await vi.waitFor(() => expect(options()).toEqual(["smarter-dev/bot"]));
  expect(document.querySelector(".repo-picker-note:not([hidden])")).toBeNull();
});

it("a bridge without the capability is never asked, and the field stays as it was", async () => {
  await greet([]);
  const call = deskCall();
  open(call);
  const input = addRemote();
  type(input, "zech");
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(reposCalls(call)).toHaveLength(0);
  expect(input.hasAttribute("role")).toBe(false);
  expect(options()).toEqual([]);
  expect(document.querySelector(".repo-picker-note:not([hidden])")).toBeNull();
});

it("a cached list paints on the picker's first paint once the app has read the machine's record", async () => {
  await greet(["github.repos"]);
  await writeCached(DEVICES_ADDRESS, [{ id: "desk", name: "Desktop" }]);
  await cacheList({ repos: REPOS, refusal: "" });
  await startGithubRepos();
  // The machine never answers: everything painted below is the cache's.
  const call = vi.fn(() => new Promise(() => {}));
  open(call);
  const input = addRemote();
  expect(input.getAttribute("role")).toBe("combobox");
  type(input, "bot");
  expect(options()).toEqual(["smarter-dev/bot"]);
});

it("reopening from a saved draft asks the machine the draft restores", async () => {
  await greet(["github.repos"]);
  await writeCached(uiAddress({ view: "new-project", kind: "draft" }), { name: "saved", sources: [], selectedDeviceId: "desk" });
  const call = deskCall();
  open(call, { defaultDeviceId: null });
  await vi.waitFor(() => expect(document.querySelector("#nrdevice").value).toBe("desk"));
  await vi.waitFor(() => expect(reposCalls(call)).toHaveLength(1));
  await vi.waitFor(async () => expect((await readCached(githubReposAddress("desk")))?.value?.repos).toEqual(REPOS));
});

it("a bridge that greets without the capability leaves a plain field even over a cached list", async () => {
  await greet([]);
  await cacheList({ repos: REPOS, refusal: "" });
  const call = deskCall();
  open(call);
  const input = addRemote();
  // Past every cache read and write the opening started.
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(input.hasAttribute("role")).toBe(false);
  type(input, "bot");
  expect(options()).toEqual([]);
  expect(key(input, "ArrowDown").defaultPrevented).toBe(false);
  expect(reposCalls(call)).toHaveLength(0);
  expect(document.querySelector(".repo-picker-note:not([hidden])")).toBeNull();
});

it("a machine that has not greeted yet still searches the list it cached", async () => {
  await cacheList({ repos: REPOS, refusal: "" });
  const call = deskCall();
  open(call);
  const input = addRemote();
  type(input, "bot");
  await vi.waitFor(() => expect(options()).toEqual(["smarter-dev/bot"]));
  expect(reposCalls(call)).toHaveLength(0);
  expect((await readCached(githubReposAddress("desk")))?.value?.repos).toEqual(REPOS);
});

// Zech, 03:07Z Sep 27: "I can't get it to work". A sheet is opened whenever the
// reader likes; the machine answering then is not a given. Each case below
// asked nothing, or cached a transport failure as the machine's refusal, and
// left the field plain for the rest of that opening.

it("a sheet opened before the machine greets asks it when it does, in the same opening", async () => {
  const call = deskCall();
  open(call);
  const input = addRemote();
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(reposCalls(call)).toHaveLength(0);
  expect(input.hasAttribute("role")).toBe(false);
  await greet(["github.repos"]);
  await vi.waitFor(() => expect(reposCalls(call)).toHaveLength(1));
  type(input, "zb");
  await vi.waitFor(() => expect(options()).toEqual(["zech/build"]));
});

it("an ask that never reached the machine caches nothing, and its next greeting asks again", async () => {
  await greet(["github.repos"]);
  let reachable = false;
  const call = vi.fn(async (method) => {
    if (method !== "github.repos") return {};
    if (reachable) return { repos: REPOS };
    throw Object.assign(new ApiError("unknown", "github.repos timed out"), { timedOut: true });
  });
  open(call);
  const input = addRemote();
  await vi.waitFor(() => expect(reposCalls(call)).toHaveLength(1));
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect((await readCached(githubReposAddress("desk")))?.value).toBeUndefined();
  expect(document.querySelector(".repo-picker-note:not([hidden])")).toBeNull();
  reachable = true;
  await greet(["github.repos"]); // the reconnect
  await vi.waitFor(() => expect(reposCalls(call)).toHaveLength(2));
  type(input, "bot");
  await vi.waitFor(() => expect(options()).toEqual(["smarter-dev/bot"]));
});

it("the machine's own refusal is settled: a later greeting does not ask again", async () => {
  await greet(["github.repos"]);
  const call = deskCall({ refuse: REFUSAL });
  open(call);
  addRemote();
  await vi.waitFor(async () => expect((await readCached(githubReposAddress("desk")))?.value?.refusal).toBe(REFUSAL));
  await greet(["github.repos"]);
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(reposCalls(call)).toHaveLength(1);
});

it("a closed sheet stops waiting: the machine greeting later is not asked", async () => {
  const call = deskCall();
  open(call);
  document.querySelector("#nrcancel").click();
  await greet(["github.repos"]);
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(reposCalls(call)).toHaveLength(0);
});
