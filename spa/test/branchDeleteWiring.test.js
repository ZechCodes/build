/** @vitest-environment jsdom */
// #87, with nothing mocked between the greeting and the call: the bridge's
// greeting (fixtures/api/v1/session.hello.json, which bridge/tests pins to the
// real reply) names `branches.finishDelete`; the real greeting path writes it
// to the real cache; the mounted branch surface reads it there, promises the
// deletion, and sends `action: "delete"` down the machine's own call — on the
// verdict of the greeting the machine's current session is on. A greeting
// without the name leaves a surface that says the bridge is too old and sends
// no action for it to drop.

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
  task_id: null,
  stat: { uncommitted: { files_changed: 0 }, ahead: 0, upstream: "origin/build/login" },
};

let App;
let call;
let context;
let stopShell;
let resetDeviceContexts;
let resetChangeEvents;

/** A session on dev-1 whose every call is `call`, as a connection adopts it. */
const sessionOnDevice = () => ({
  deviceId: "dev-1",
  call: (...args) => call(...args),
  close: () => {},
  peer: () => {},
  onCarrier: () => {},
});

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  document.body.innerHTML = bodyHtml;
  location.hash = "#/p/p1/branch/build%2Flogin/changes";
  ({ App } = await import("../src/app.js"));
  ({ resetChangeEvents } = await import("../src/core/changeEvents.js"));
  resetChangeEvents();
  const { adoptDeviceSession, ...contexts } = await import("../src/core/deviceContexts.js");
  resetDeviceContexts = contexts.resetDeviceContexts;
  App.devices = [{ id: "dev-1", name: "studio", status: "online" }];
  const { DEVICES_ADDRESS, writeCached } = await import("../src/core/localCache.js");
  await writeCached(DEVICES_ADDRESS, App.devices);
  await (await import("../src/devices.js")).readCachedDevices();
  App.selectedDeviceId = "dev-1";
  call = vi.fn(async () => ({}));
  context = adoptDeviceSession(sessionOnDevice());
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

/** Greet the device's session the way connection.js does: this hello's own
 *  greeting authority, the adapter it selects installed on the device's
 *  context, then published to what reads capabilities. `hello` may be a
 *  promise, held to keep the greeting pending. */
async function greetSession(hello) {
  const { adoptBridgeSelection, greetingInFlight } = await import("../src/core/deviceContexts.js");
  const { greetBridge } = await import("../src/core/changeEvents.js");
  const authority = greetingInFlight(context);
  const rawCall = async (method) => (method === "session.hello" ? hello : {});
  return greetBridge(rawCall, {
    deviceId: "dev-1",
    strict: true,
    isCurrent: () => authority.current(),
    install: (selection) => {
      const adapter = selection.unsupported ? null : selection.create(rawCall);
      adoptBridgeSelection(context, selection, adapter, authority);
      return adapter;
    },
  });
}

/** Greet the bridge the way a connection does, then stand the surface on the
 *  cached row. */
async function greetThenOpen(hello) {
  const { readBranchDelete } = await import("../src/core/branchDeleteSupport.js");
  await greetSession(hello);
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

// The bridge measured the branch again once the checkout was gone and kept
// it: the workspace is gone and stays gone, and the user reads why the branch
// did not go with it.
it("a branch the bridge kept after the removal is said, and the removal stands", async () => {
  const reason = "Build cannot delete the branch build/login: it gained commits while Build was deleting it.";
  call = vi.fn(async (method) =>
    method === "branch.finish" ? { complete: true, repositories: [], deleted: true, branch_deleted: false, branch_reason: reason } : {},
  );
  await greetThenOpen(greeting);
  await pressDone();
  const notice = await vi.waitFor(() => {
    const found = document.querySelector("#notices .notice");
    expect(found).toBeTruthy();
    return found;
  });
  expect(notice.textContent).toContain("Removed the checkout of build/login; the branch stays");
  expect(notice.textContent).toContain(reason);
  expect(notice.textContent).not.toContain("Couldn't finish");
});

it("a failed branch recovery leaves a lasting notice without claiming the branch stayed", async () => {
  const reason = "Build could not restore branch build/login at abc in /repo after it was deleted.";
  call = vi.fn(async (method) =>
    method === "branch.finish" ? { complete: true, repositories: [], deleted: true, branch_deleted: true, branch_reason: reason } : {},
  );
  await greetThenOpen(greeting);
  await pressDone();
  const notice = await vi.waitFor(() => {
    const found = document.querySelector("#notices .notice.error");
    expect(found).toBeTruthy();
    return found;
  });
  expect(notice.textContent).toContain("branch recovery failed");
  expect(notice.textContent).not.toContain("branch stays");
  expect(notice.querySelector(".notice-detail").textContent).toContain(reason);
});

it("a bridge without the name is told nothing it would drop, and the user is told why", async () => {
  await greetThenOpen(olderGreeting);
  const { text, params } = await pressDone();
  expect(text).toContain("Build cannot delete the branch on studio: the bridge is too old.");
  expect(text).not.toContain("Delete branch");
  expect(params).toEqual({ project_id: "p1", branch: "build/login" });
});

it("a deletion confirmed against the cache is refused in words when the bridge answering now keeps the branch", async () => {
  await greetSession(olderGreeting);
  const { finishWorkItem } = await import("../src/core/inboxView.js");
  await expect(
    finishWorkItem({ kind: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login", deletesBranch: true, deviceName: "studio" }),
  ).rejects.toThrow("Build cannot delete the branch on studio: the bridge is too old.");
  expect(call.mock.calls.some(([method]) => method === "branch.finish")).toBe(false);
});

// The cache said this machine deletes, and the confirmation promised it. Then
// the machine came back on a new session to an older bridge whose hello has
// not answered: the deletion waits on that greeting, and is refused in words
// once it says the bridge keeps the branch — never sent for it to drop.
it("a deletion waits for the current session's greeting, and an older bridge answering it is never sent the word", async () => {
  await greetThenOpen(greeting);
  const { adoptDeviceSession } = await import("../src/core/deviceContexts.js");
  expect(adoptDeviceSession(sessionOnDevice())).toBe(context);
  let answerHello;
  const greeted = greetSession(new Promise((answer) => { answerHello = answer; }));

  document.querySelector("#tb-verb .btn.mini:not(.caret)").click();
  const scrim = await vi.waitFor(() => {
    const found = document.getElementById("confirm-scrim");
    expect(found).toBeTruthy();
    return found;
  });
  expect(scrim.textContent).toContain("Done deletes the branch. This cannot be undone.");
  scrim.querySelector("[data-confirm-ok]").click();
  for (let index = 0; index < 12; index += 1) await flush();
  expect(call.mock.calls.some(([method]) => method === "branch.finish")).toBe(false);

  answerHello(olderGreeting);
  await greeted;
  const notice = await vi.waitFor(() => {
    const found = document.querySelector("#notices .notice");
    expect(found).toBeTruthy();
    return found;
  });
  expect(notice.textContent).toContain("Build cannot delete the branch on studio: the bridge is too old.");
  expect(call.mock.calls.some(([method]) => method === "branch.finish")).toBe(false);
});
