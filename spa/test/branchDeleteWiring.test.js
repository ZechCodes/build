/** @vitest-environment jsdom */
// #87, with nothing mocked between the greeting and the call: the bridge's
// greeting (fixtures/api/v1/session.hello.json, which bridge/tests pins to the
// real reply) names `branches.finishDelete`; the real greeting path writes it
// to the real cache; the mounted branch surface reads it there, promises the
// deletion, and sends `action: "delete"` down the machine's own call. A
// greeting without the name leaves a surface that says the bridge is too old
// and sends no action for it to drop.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const greeting = JSON.parse(readFileSync(resolve(process.cwd(), "../fixtures/api/v1/session.hello.json"), "utf8")).result;
const olderGreeting = { ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "branches.finishDelete") };
const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const flush = () => new Promise((done) => setTimeout(done, 0));

const branchRow = {
  kind: "branch",
  project_id: "p1",
  project: "relaydb",
  branch: "build/login",
  worktree_id: "wt-1",
  agents: [],
  state: "review",
  can_finish: true,
  finish: { warnings: [] },
  primary: false,
  issue_id: null,
  stat: { uncommitted: { files_changed: 0 }, ahead: 0, upstream: "origin/build/login" },
};

let App;
let call;
let stopShell;
let resetDeviceContexts;
let resetChangeEvents;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  document.body.innerHTML = bodyHtml;
  location.hash = "#/p/p1/branch/build%2Flogin/changes";
  ({ App } = await import("../src/app.js"));
  ({ resetChangeEvents } = await import("../src/core/changeEvents.js"));
  resetChangeEvents();
  const { adoptBridgeSelection, adoptDeviceSession, ...contexts } = await import("../src/core/deviceContexts.js");
  resetDeviceContexts = contexts.resetDeviceContexts;
  App.devices = [{ id: "dev-1", name: "studio", status: "online" }];
  const { DEVICES_ADDRESS, writeCached } = await import("../src/core/localCache.js");
  await writeCached(DEVICES_ADDRESS, App.devices);
  await (await import("../src/devices.js")).readCachedDevices();
  App.selectedDeviceId = "dev-1";
  call = vi.fn(async () => ({}));
  const context = adoptDeviceSession({
    deviceId: "dev-1",
    call: (...args) => call(...args),
    close: () => {},
    peer: () => {},
    onCarrier: () => {},
  });
  adoptBridgeSelection(context, { major: 1, version: greeting.api_version }, {});
  App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login", tab: "changes" };
  document.getElementById("toolbar").innerHTML = '<span id="tb-verb"></span>';
  let standShell;
  ({ standShell, stopShell } = await import("../src/core/shell.js"));
  App.standShell = standShell;
});

afterEach(() => {
  stopShell();
  resetDeviceContexts();
  resetChangeEvents();
  if (App.poll) clearInterval(App.poll);
  App.poll = null;
  if (App.viewDispose) App.viewDispose();
  App.viewDispose = null;
});

/** Greet the bridge the way a connection does, then stand the surface on the
 *  cached row. */
async function greetThenOpen(hello) {
  const { greetBridge } = await import("../src/core/changeEvents.js");
  const { readBranchDelete } = await import("../src/core/branchDeleteSupport.js");
  await greetBridge(async (method) => (method === "session.hello" ? hello : {}), { deviceId: "dev-1", strict: true });
  const expected = hello.capabilities.includes("branches.finishDelete");
  await vi.waitFor(async () => expect(await readBranchDelete("dev-1")).toBe(expected));

  const { writeCached } = await import("../src/core/localCache.js");
  const { liveFeedSnapshot } = await import("../src/core/feedMerge.js");
  const { entityIdOf } = await import("../src/core/entityId.js");
  const { startFeed } = await import("../src/core/taskFeed.js");
  const projects = [{ project_id: "p1", name: "notes", is_git: true }];
  const view = liveFeedSnapshot({ items: [branchRow], runs: [] }, { projects }, { workspaces: [] }, "dev-1");
  await writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, view);
  await writeCached({ deviceId: "dev-1", entityId: "", kind: "projects" }, view.projects);
  await writeCached({ deviceId: "dev-1", entityId: "", kind: "workspaces" }, view.workspaces);
  for (const item of view.items) await writeCached({ deviceId: "dev-1", entityId: entityIdOf(item), kind: "row" }, item);
  await startFeed();
  for (let index = 0; index < 12; index += 1) await flush();

  App.standShell(App.route);
  const { renderBranch } = await import("../src/views/branchView.js");
  await renderBranch();
  await flush();
}

/** Press Done, read the confirmation, confirm it, and answer what was sent. */
async function pressDone() {
  document.querySelector("#tb-verb .btn.mini:not(.caret)").click();
  const scrim = await vi.waitFor(() => {
    const found = document.getElementById("confirm-scrim");
    expect(found).toBeTruthy();
    return found;
  });
  const text = scrim.textContent;
  scrim.querySelector("[data-confirm-ok]").click();
  await vi.waitFor(() => expect(call.mock.calls.some(([method]) => method === "branch.finish")).toBe(true));
  return { text, params: call.mock.calls.find(([method]) => method === "branch.finish")[1] };
}

it("a bridge announcing branches.finishDelete is promised the deletion and asked for it", async () => {
  expect(greeting.capabilities).toContain("branches.finishDelete");
  await greetThenOpen(greeting);
  const { text, params } = await pressDone();
  expect(text).toContain("Done deletes the branch. This cannot be undone.");
  expect(text).toContain("Delete branch build/login");
  expect(params).toEqual({ project_id: "p1", branch: "build/login", action: "delete" });
});

it("a bridge without the name is told nothing it would drop, and the user is told why", async () => {
  await greetThenOpen(olderGreeting);
  const { text, params } = await pressDone();
  expect(text).toContain("Build cannot delete the branch on studio: the bridge is too old.");
  expect(text).not.toContain("Delete branch");
  expect(params).toEqual({ project_id: "p1", branch: "build/login" });
});

it("a deletion confirmed against the cache is refused in words when the bridge answering now keeps the branch", async () => {
  const { greetBridge } = await import("../src/core/changeEvents.js");
  await greetBridge(async (method) => (method === "session.hello" ? olderGreeting : {}), { deviceId: "dev-1", strict: true });
  const { finishWorkItem } = await import("../src/core/inboxView.js");
  await expect(
    finishWorkItem({ kind: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login", deletesBranch: true, deviceName: "studio" }),
  ).rejects.toThrow("Build cannot delete the branch on studio: the bridge is too old.");
  expect(call.mock.calls.some(([method]) => method === "branch.finish")).toBe(false);
});
