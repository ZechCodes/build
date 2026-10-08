// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import "fake-indexeddb/auto";
import { IDBDatabase } from "fake-indexeddb";
import { readCached, wipeCache } from "../src/core/localCache.js";
import { clearedWorkspaceReviewTask, replaceSessionList, sessionListObservation, upsertSessionRow } from "../src/core/sessionListCache.js";

const view = vi.hoisted(() => ({ App: {}, context: null, workspace: null, initialization: null }));
vi.mock("../src/app.js", () => ({ App: view.App, go: vi.fn(), markRoute: vi.fn() }));
vi.mock("../src/core/shell.js", () => ({ shellSelection: () => ({}) }));
vi.mock("../src/core/surfaceContext.js", () => ({ surfaceContext: () => view.context }));
vi.mock("../src/core/deviceContexts.js", () => ({ routeContext: () => view.context }));
vi.mock("../src/core/deviceNotice.js", () => ({ mountDeviceNotice: () => {}, mountDeviceStrip: () => () => {} }));
vi.mock("../src/core/feedRows.js", () => ({ deviceFeedNow: () => ({ workspaces: [view.workspace] }) }));
vi.mock("../src/core/cachedRows.js", () => ({ cachedFeedView: async () => ({ workspaces: [view.workspace] }) }));
vi.mock("../src/core/taskFeed.js", () => ({ subscribeFeed: (listener) => { listener(); return () => {}; }, refreshFeed: async () => [true] }));
vi.mock("../src/core/workspaceRail.js", () => ({ mountWorkspaceRail: () => ({ paint() {}, feedMoved() {}, dispose() {} }) }));
vi.mock("../src/views/files.js", () => ({ renderFilesTab: () => ({ dispose() {} }) }));
vi.mock("../src/core/workspaceReviewEntry.js", () => ({ mountWorkspaceReviewEntry: () => ({ dispose() {} }) }));
vi.mock("../src/core/workspaceTasksTab.js", () => ({ mountWorkspaceTasksTab: () => ({ dispose() {} }), workspaceTasksPlace: () => ({}) }));
vi.mock("../src/views/workspaceChanges.js", () => ({ mountWorkspaceChanges: (host) => {
  host.innerHTML = '<div class="workspace-changes"><div class="workspace-changes-surface"><aside class="crail-host"></aside></div></div>';
  return { dispose() {}, workspaceMoved() {} };
} }));
vi.mock("../src/core/workspaceGitInitialization.js", () => ({ mountWorkspaceGitInitialization: (options) => {
  view.initialization = options;
  const button = document.createElement("button");
  button.dataset.initGit = "";
  options.host.appendChild(button);
  return { dispose() {}, setVisible() {} };
} }));
import { renderWorkspace } from "../src/views/workspaceView.js";

const address = { deviceId: "workspace-review-clear", entityId: "", kind: "workspaces" };
const authority = { clearMissingReview: true };
const summary = (version, taskId = "pr-1") => ({ task_id: taskId, workspace_id: "ws-1", version, status: "open" });
const workspace = (version, taskId = "pr-1") => ({ id: "ws-1", project_id: "project-1", name: "Workspace", active_review: summary(version, taskId) });
const missing = { id: "ws-1", project_id: "project-1", name: "Workspace without PR" };
const read = async () => (await readCached(address)).value;
const push = (rows) => replaceSessionList(address, "workspaces", rows, undefined, undefined, authority);
beforeEach(async () => { await wipeCache(); });
afterEach(() => { view.App.viewDispose?.(); view.App.viewDispose = null; document.body.innerHTML = ""; vi.restoreAllMocks(); });

function holdWorkspaceWrite() {
  const started = Promise.withResolvers();
  let release;
  const transaction = IDBDatabase.prototype.transaction;
  const spy = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    const opened = transaction.apply(this, args);
    if (this.name === "build-cache" && args[1] === "readwrite") {
      spy.mockRestore();
      const start = opened._start.bind(opened);
      opened._start = () => started.resolve();
      release = () => { opened._start = start; start(); };
    }
    return opened;
  });
  return { started: started.promise, release: () => release() };
}

it.each(["push", "pull"])("rejects a queued workspace %s replacement after the context stops", async (source) => {
  await replaceSessionList(address, "workspaces", [workspace(7)]);
  const held = await readCached(address);
  const observation = source === "pull" ? await sessionListObservation(address, "workspaces") : undefined;
  let active = true;
  const replaced = vi.fn();
  const gate = holdWorkspaceWrite();
  const pending = replaceSessionList(address, "workspaces", [missing], replaced, observation,
    { ...authority, active: () => active });
  await gate.started;
  expect(replaced).not.toHaveBeenCalled();
  active = false;
  gate.release();
  expect(await pending).toBe(false);
  expect(replaced).not.toHaveBeenCalled();
  expect(await readCached(address)).toEqual(held);
});

it.each(["push", "pull"])("accepts a queued workspace %s replacement while the context remains active", async (source) => {
  await replaceSessionList(address, "workspaces", [workspace(7)]);
  const observation = source === "pull" ? await sessionListObservation(address, "workspaces") : undefined;
  const replaced = vi.fn();
  const gate = holdWorkspaceWrite();
  const pending = replaceSessionList(address, "workspaces", [missing], replaced, observation,
    { ...authority, active: () => true });
  await gate.started;
  expect(replaced).not.toHaveBeenCalled();
  gate.release();
  expect(await pending).toBe(true);
  expect(replaced).toHaveBeenCalledTimes(1);
  const held = (await read())[0];
  expect(held).not.toHaveProperty("active_review");
  expect(clearedWorkspaceReviewTask(held)).toBe("pr-1");
});

it("allows an authoritative board push to clear a summary without a list-request observation", async () => {
  await replaceSessionList(address, "workspaces", [workspace(7)]);
  await push([missing]);
  const held = (await read())[0];
  expect(held).not.toHaveProperty("active_review");
  expect(clearedWorkspaceReviewTask(held)).toBe("pr-1");
});

it("allows a full workspace detail result to clear an omitted summary while preserving other row fields", async () => {
  const other = { id: "ws-2", project_id: "project-1", name: "Other workspace" };
  await replaceSessionList(address, "workspaces", [{ ...workspace(7), directories: [{ id: "dir-1" }] }, other]);
  const observation = await sessionListObservation(address, "workspaces");
  await upsertSessionRow(address, "workspaces", missing, () => true, { ...authority, observation });
  const held = await read();
  expect(held[0]).toMatchObject({ name: missing.name, directories: [{ id: "dir-1" }] });
  expect(held[0]).not.toHaveProperty("active_review");
  expect(clearedWorkspaceReviewTask(held[0])).toBe("pr-1");
  expect(held[1]).toEqual(other);
});

it("keeps a summary created after a delayed detail request began", async () => {
  await replaceSessionList(address, "workspaces", [workspace(7)]);
  const observation = await sessionListObservation(address, "workspaces");
  await push([workspace(8)]);
  await upsertSessionRow(address, "workspaces", missing, () => true, { ...authority, observation });
  expect((await read())[0].active_review).toEqual(summary(8));
});

it("blocks delayed list and detail replies from restoring a summary cleared by a push", async () => {
  await replaceSessionList(address, "workspaces", [workspace(7)]);
  const observation = await sessionListObservation(address, "workspaces");
  await push([missing]);
  await replaceSessionList(address, "workspaces", [workspace(3)], undefined, observation);
  await upsertSessionRow(address, "workspaces", workspace(7), () => true, { ...authority, observation });
  const held = (await read())[0];
  expect(held).not.toHaveProperty("active_review");
  expect(clearedWorkspaceReviewTask(held)).toBe("pr-1");
});

it("preserves the PR version floor across authoritative writers and lets a strictly newer version advance", async () => {
  await replaceSessionList(address, "workspaces", [workspace(7)]);
  await push([workspace(3)]);
  expect((await read())[0].active_review).toEqual(summary(7));
  await push([missing]);
  await push([workspace(3)]);
  expect((await read())[0]).not.toHaveProperty("active_review");
  await push([workspace(8)]);
  expect((await read())[0].active_review).toEqual(summary(8));
});

it("lets a new PR reuse the workspace while protecting it from old list and detail replies", async () => {
  await replaceSessionList(address, "workspaces", [workspace(7)]);
  const observation = await sessionListObservation(address, "workspaces");
  await push([workspace(1, "pr-2")]);
  await replaceSessionList(address, "workspaces", [workspace(7)], undefined, observation);
  await upsertSessionRow(address, "workspaces", missing, () => true, { ...authority, observation });
  expect((await read())[0].active_review).toEqual(summary(1, "pr-2"));
});

it("keeps a held review for a partial upsert that has no clear authority", async () => {
  await replaceSessionList(address, "workspaces", [workspace(7)]);
  await upsertSessionRow(address, "workspaces", missing);
  expect((await read())[0].active_review).toEqual(summary(7));
});

async function mountFailedWorkspace(callRpc, isGit = true) {
  const held = { ...workspace(7), status: "failed", directories: [{ id: "dir-1", source_id: "dir-1", name: "Repository", is_git: isGit }] };
  await replaceSessionList(address, "workspaces", [held]);
  view.workspace = held;
  view.App.route = { name: "workspace", deviceId: address.deviceId, projectId: "project-1", workspaceId: "ws-1", sourceId: "dir-1", tab: "changes" };
  view.context = { deviceId: address.deviceId, rpc: callRpc, active: () => true,
    cacheScope: { address: (parts) => ({ ...parts, deviceId: address.deviceId }) } };
  document.body.innerHTML = '<nav id="dir-rail"></nav><div id="root"></div>';
  await renderWorkspace();
  return held;
}

it("the real workspace Retry result clears an omitted active_review through the cache writer", async () => {
  const callRpc = vi.fn(async (method) => method === "workspace.retry"
    ? { workspace: { ...missing, status: "ready", directories: [{ id: "dir-1", source_id: "dir-1", is_git: true }] } }
    : { source: { is_git: true } });
  await mountFailedWorkspace(callRpc);
  const retry = document.querySelector("[data-workspace-action]");
  expect(retry).not.toBeNull();
  await retry.onclick();
  expect((await read())[0]).toMatchObject({ status: "ready" });
  expect((await read())[0]).not.toHaveProperty("active_review");
  expect(clearedWorkspaceReviewTask((await read())[0])).toBe("pr-1");
});

it("a late real Retry result cannot clear a PR that started while its request was in flight", async () => {
  const started = Promise.withResolvers();
  const answer = Promise.withResolvers();
  const callRpc = vi.fn(async (method) => {
    if (method !== "workspace.retry") return { source: { is_git: true } };
    started.resolve();
    return answer.promise;
  });
  const held = await mountFailedWorkspace(callRpc);
  const retried = document.querySelector("[data-workspace-action]").onclick();
  await started.promise;
  await push([{ ...held, active_review: summary(1, "pr-2") }]);
  answer.resolve({ workspace: { ...missing, status: "ready" } });
  await retried;
  expect((await read())[0].active_review).toEqual(summary(1, "pr-2"));
});

it("the Git initialization result callback clears an omitted review with captured request authority", async () => {
  const answer = { workspace: { ...missing, status: "ready" }, results: [] };
  const callRpc = vi.fn(async () => answer);
  await mountFailedWorkspace(callRpc, false);
  expect(view.initialization).not.toBeNull();
  const result = await view.initialization.callRpc("workspace.init_git", { workspace_id: "ws-1", source_id: "dir-1", target: "workspace" });
  await view.initialization.onUpdate(result);
  expect((await read())[0]).not.toHaveProperty("active_review");
});
