// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createViewingContext } from "../src/core/viewingContext.js";
import { createAgentSelection } from "../src/core/agentSelection.js";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { readUiRecord, writeUiRecord, wipeUiRecords } from "../src/core/localUiStore.js";
import { rememberProjectChangesSupport } from "../src/core/projectChangesSupport.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { writeReviewRecord } from "../src/core/taskReviewCache.js";
import { writeTaskRecord, writeTasksRecord } from "../src/core/trackerCache.js";
import { directoryCacheId } from "../src/core/directoryScope.js";
import { worktreeOf } from "./gitWireFixture.js";
import { columns, task } from "./trackerWireFixture.js";
import reviewFixture from "../../fixtures/api/v1/tasks.review.open.json";

const runtime = vi.hoisted(() => ({ context: null, feed: null, listeners: new Set(), capabilities: {}, failure: null }));
vi.mock("../src/core/surfaceContext.js", () => ({ surfaceContext: () => runtime.context }));
vi.mock("../src/core/taskFeed.js", async original => ({ ...await original(), subscribeFeed: fn => {
  runtime.listeners.add(fn); fn(runtime.feed); return () => runtime.listeners.delete(fn);
}, refreshFeed: async () => [] }));
vi.mock("../src/core/changeEvents.js", async original => ({ ...await original(),
  bridgeCapabilities: () => runtime.capabilities, onBridgeGreeted: () => () => {},
  watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceContexts.js", async original => ({ ...await original(), contextFor: () => runtime.context,
  captureDeviceGreeting: () => ({ epoch: "test-epoch", current: () => true }),
  whenGreeted: async (_context, send) => ({ sent: send() }),
}));
vi.mock("../src/core/deviceReconnect.js", async original => ({ ...await original(), deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/projectRail.js", () => ({ mountProjectRail: () => ({ paint() {}, dispose() {} }) }));
vi.mock("../src/core/deviceNotice.js", () => ({ mountDeviceStrip: () => () => {}, mountDeviceNotice() {} }));
vi.mock("../src/core/toolbar.js", async original => ({ ...await original(),
  setToolbarVerb: paint => paint(document.querySelector("#tb-verb")), clearToolbarVerb() {},
}));
vi.mock("../src/core/shell.js", async original => ({ ...await original(), shellSelection: () => createAgentSelection(), collapseChatOverPage() {} }));

import { App } from "../src/app.js";
import { mountChanges } from "../src/views/workspaceChanges.js";
import { mountArchivedProjectView } from "../src/core/archivedProjectView.js";
import { hideCommentPop } from "../src/commentPop.js";
import { renderProject } from "../src/views/projectView.js";
import { renderTrackerTask } from "../src/views/trackerTaskView.js";

const deviceId = "archive-device", projectId = "archive-project", taskId = "task-archive";
const address = { deviceId, entityId: "", kind: "projects" };
const source = { id: "source-1", name: "Code", path: "/work/code", is_git: true, base_branch: "main" };
const project = { id: projectId, project_id: projectId, name: "Old line", deviceId, projectKey: `${deviceId}/${projectId}`,
  archived_at: 12, environment_id: "env-1", added_sources: [source], sources: [source], path: source.path };
const scope = { deviceId, projectId, taskId };
const detail = { task: task({ id: taskId, project_id: projectId }), timeline: [] };
let callRpc;
const extraViews = [];

async function cacheProject(row) {
  await writeCached(address, [row]);
  runtime.feed = { ...runtime.feed, projects: [row] };
  runtime.listeners.forEach(fn => fn(runtime.feed));
}
beforeEach(async () => {
  await wipeCache(); await wipeUiRecords(); runtime.listeners.clear(); runtime.failure = null;
  document.body.innerHTML = '<div id="tb-verb"></div><nav id="dir-rail"></nav><main id="root"></main><aside id="agent-rail"></aside>';
  runtime.capabilities = { environments: { archive: true }, projects: { syncBase: true, syncBasePath: true }, git: { projectSources: true },
    changes: { subscriptions: true, kinds: ["state", "tasks", "git", "files"] }, tasks: { attachments: true, watching: true } };
  callRpc = vi.fn(async (method, params) => {
    if (runtime.failure?.method === method) {
      const failure = runtime.failure;
      await failure.wait;
      throw Object.assign(new Error(failure.message), { code: "project_archived" });
    }
    if (method === "project.restore") {
      // The feed may observe restore before the archive notice's own reader.
      runtime.feed = { ...runtime.feed, projects: [{ ...project, archived_at: null }] };
      runtime.listeners.forEach(fn => fn(runtime.feed));
      return { project: { ...project, archived_at: null } };
    }
    // Unanswered reads must not prevent any cached view from mounting.
    return new Promise(() => {});
  });
  runtime.context = { deviceId, rpc: callRpc, active: () => true, modelCatalog: () => ({ providers: [] }), refreshModelCatalog: async () => ({ providers: [] }),
    cacheScope: { deviceId, address: a => ({ deviceId, ...a }) } };
  runtime.feed = { projects: [project], workspaces: [], items: [], runs: [], pending: [], devices: {} };
  App.devices = [{ id: deviceId, name: "Device" }]; App.viewingContext = createViewingContext();
  await writeCached({ ...address, kind: "environment-support" }, { list: true, archive: true, projectRename: true, environmentRename: true, addedSources: true, bridge_epoch: "test-epoch" });
  await rememberProjectChangesSupport(deviceId, runtime.capabilities);
  await writeTasksRecord(deviceId, projectId, { tasks: [detail.task], columns: columns() });
  await writeTaskRecord(deviceId, projectId, taskId, detail);
  const entityId = directoryCacheId({ project_id: projectId, source_id: source.id, source_path: source.path });
  await writeCached({ deviceId, entityId, kind: "status" }, worktreeOf({ "a.js": "changed" }).status({ ahead: 1, behind: 1, stash_count: 1 }));
  await writeCached({ deviceId, entityId, kind: "log" }, { commits: [], more: false });
  await writeCached({ deviceId, entityId, kind: "refs" }, { refs: [{ kind: "branch", name: "main", full_ref: "refs/heads/main", current: true }, { kind: "branch", name: "other", full_ref: "refs/heads/other" }] });
  await cacheProject(project);
});
afterEach(() => { extraViews.splice(0).forEach(view => view.dispose()); hideCommentPop(); window.getSelection()?.removeAllRanges(); App.viewDispose?.(); App.viewDispose = null; });
async function mountProject() {
  App.route = { name: "project", deviceId, projectId, tab: "changes" };
  await renderProject();
  await vi.waitFor(() => expect(document.querySelector("[data-project-sync-base]")?.hidden).toBe(false));
}
async function mountTask(review = false) {
  if (review) {
    const saved = { ...reviewFixture.result.review, task_id: taskId, snapshots: [{ id: "snapshot", number: 1, directories: [] }] };
    await rememberReviewSupport(deviceId, { reviews: { get: true, comments: true, pullRequests: true, complete: true, close: true, refresh: true } });
    await writeReviewRecord(scope, saved, 1);
  }
  App.route = { name: "trackerTask", ...scope };
  await renderTrackerTask();
  await vi.waitFor(() => expect(document.querySelector(review ? "[data-review-pending]" : ".task-page-title")).not.toBeNull());
}
it("archived project view shows the banner with restore", async () => {
  await mountProject();
  await vi.waitFor(() => expect(document.querySelector("[data-project-archived]")?.textContent).toContain("Old line is archived"));
  expect(document.querySelector("[data-project-sync-base]").disabled).toBe(true);
  for (const selector of [".gtfetch", ".gtsync .btn", ".gtstash .btn", ".csbox textarea", ".csbox-actions .btn", "[data-ref]"]) {
    await vi.waitFor(() => expect(document.querySelector(selector)?.disabled, selector).toBe(true));
  }
  document.querySelector("[data-project-sync-base]").click();
  expect(callRpc.mock.calls.some(([method]) => method === "project.sync_source")).toBe(false);
  document.querySelector("[data-project-restore]").click();
  await vi.waitFor(async () => expect((await readCached(address)).value[0].archived_at).toBeNull());
  await vi.waitFor(() => expect(document.querySelector("[data-project-archived]")).toBeNull());
  expect(callRpc).toHaveBeenCalledWith("project.restore", { project_id: projectId });
  await vi.waitFor(() => expect(document.querySelector("[data-project-sync-base]").disabled).toBe(false));
});
it("archived task and review views disable mutations", async () => {
  await mountTask();
  await vi.waitFor(() => expect(document.querySelector("[data-task-state]")?.disabled).toBe(true));
  for (const selector of ["#task-status", "#task-priority", "#task-labels", "[data-task-assign]", "textarea"]) expect(document.querySelector(selector)?.disabled).toBe(true);
  document.querySelector("[data-task-state]").click();
  expect(callRpc.mock.calls.some(([method]) => method === "tasks.close")).toBe(false);
  App.viewDispose(); App.viewDispose = null;
  await mountTask(true);
  await vi.waitFor(() => expect(document.querySelector("[data-review-verdict=approve]")?.disabled).toBe(true));
  expect(document.querySelector("[data-review-summary]").disabled).toBe(true);
  expect(document.querySelector("[data-review-close] button").disabled).toBe(true);
  expect(document.querySelector("[data-review-repair]").disabled).toBe(true);
  expect(document.querySelector("[data-review-comment-batch]").disabled).toBe(true);
  expect(document.querySelector("[data-review-view=files]").disabled).toBe(false);
});
it("project_archived refusal is shown verbatim", async () => {
  await mountProject(); // Direct archived links paint before any bridge read.
  await cacheProject({ ...project, archived_at: null });
  await vi.waitFor(() => expect(document.querySelector("[data-project-sync-base]").disabled).toBe(false));
  // A write started after restore can race another client's archive.
  const message = "Old <line> is archived; restore it first";
  let release;
  runtime.failure = { method: "project.sync_source", message, wait: new Promise(resolve => { release = resolve; }) };
  document.querySelector("[data-project-sync-base]").click();
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("project.sync_source", expect.objectContaining({ project_id: projectId, source_id: source.id })));
  await cacheProject(project);
  await vi.waitFor(() => expect(document.querySelector("[data-project-archived]")).not.toBeNull());
  release();
  await vi.waitFor(() => expect(document.querySelector("[data-project-sync-error]").textContent).toBe(message));
  expect(document.querySelector("line")).toBeNull();
});

// Bare project sources have no conversation. The shared Changes view also
// reads historical run/workspace checkouts, whose real comment layer does.
async function mountCommentingChanges(archived = true) {
  await cacheProject({ ...project, archived_at: archived ? 12 : null });
  const entityId = "historical-run";
  const draftAddress = { deviceId, entityId, kind: "ui-draft", sub: "changes:inline-comments" };
  const comments = [{ id: 1, file: "a.js", lnA: 2, lnB: 2, snippet: "changed", comment: "Keep this note", side: "new" }];
  await writeUiRecord(draftAddress, { comments });
  const tree = worktreeOf({ "a.js": "changed" });
  await writeCached({ deviceId, entityId, kind: "status" }, tree.status());
  await writeCached({ deviceId, entityId, kind: "log" }, { commits: [], more: false });
  const root = document.querySelector("#root"), body = document.createElement("section"); root.append(body);
  extraViews.push(mountArchivedProjectView(root, { deviceId, projectId, callRpc }));
  const view = mountChanges(body, { scope: { run_id: entityId }, cacheScope: runtime.context.cacheScope,
    callRpc: (method, params) => method === "git.diff" ? Promise.resolve(tree.diff(params)) : callRpc(method, params),
    viewingContext: App.viewingContext,
  });
  extraViews.push(view);
  await vi.waitFor(() => expect(body.querySelector("tr[data-ln]:not(.hunk) .code")).not.toBeNull());
  await vi.waitFor(() => expect(body.querySelector(".pcx")).not.toBeNull());
  body.querySelector(".diff-expand")?.click();
  await vi.waitFor(() => expect(body.querySelector(".file.capped")).toBeNull());
  return { body, draftAddress, comments, view };
}
it("archived real Changes locks gutter and selection notes and preserves saved drafts", async () => {
  const { body, draftAddress, comments } = await mountCommentingChanges();
  const code = body.querySelector("tr[data-ln]:not(.hunk) .code");
  code.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
  await vi.waitFor(() => expect(body.querySelector(".dcmt")?.disabled).toBe(true));
  expect(body.querySelector(".pcx").getAttribute("aria-disabled")).toBe("true");
  expect(body.querySelector(".cscancel").disabled).toBe(true);
  body.querySelector(".dcmt").click(); code.click();
  expect(document.querySelector(".comment-pop")).toBeNull();
  body.querySelector(".pcx").click();
  body.querySelector(".pcx").onclick(); // A retained handler must also refuse.
  body.querySelector(".cscancel").click();
  const range = document.createRange(); range.selectNodeContents(code);
  range.getBoundingClientRect = () => ({ left: 0, bottom: 20 });
  window.getSelection().removeAllRanges(); window.getSelection().addRange(range);
  document.dispatchEvent(new Event("selectionchange"));
  await new Promise(resolve => setTimeout(resolve, 400));
  expect(document.querySelector(".comment-pop")).toBeNull();
  expect((await readUiRecord(draftAddress)).value.comments).toEqual(comments);
});
it.each([false, true])("a cached archive closes a real Changes popup without saving its draft (hidden=%s)", async hidden => {
  const { body, draftAddress, comments, view } = await mountCommentingChanges(false);
  const code = body.querySelector("tr[data-ln]:not(.hunk) .code");
  code.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
  body.querySelector(".dcmt").click();
  await vi.waitFor(() => expect(document.querySelector(".cp-input")).not.toBeNull());
  document.querySelector(".cp-input").value = "Cannot save after archive";
  const save = document.querySelector(".cp-save").onclick;
  if (hidden) view.setVisible(false);
  await cacheProject(project);
  await vi.waitFor(() => expect(document.querySelector("#root").getAttribute("data-project-archive-state")).toBe("archived"));
  if (hidden) view.setVisible(true);
  await vi.waitFor(() => expect(document.querySelector(".comment-pop")).toBeNull());
  save();
  expect((await readUiRecord(draftAddress)).value.comments).toEqual(comments);
});
