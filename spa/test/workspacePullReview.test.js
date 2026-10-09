// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const observation = vi.hoisted(() => ({ writer: null, contextRead: null }));
vi.mock("../src/core/taskReviewDrafts.js", async (importOriginal) => {
  const actual = await importOriginal();
  return { ...actual, watchReviewCreateDraft(...args) {
    observation.writer = actual.watchReviewCreateDraft(...args);
    return observation.writer;
  } };
});
vi.mock("../src/core/workspaceReviewState.js", async (importOriginal) => {
  const actual = await importOriginal();
  return { ...actual, readWorkspaceReviewContext(...args) {
    observation.contextRead = actual.readWorkspaceReviewContext(...args);
    return observation.contextRead;
  } };
});

import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { wipeUiRecords, readUiRecord } from "../src/core/localUiStore.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { reviewCreateDraftAddress } from "../src/core/taskReviewDrafts.js";
import { formatReviewSinceNote, pendingReviewText } from "../src/core/workspaceReviewState.js";
import { writeTaskRecord, writeTasksRecord, taskRecord } from "../src/core/trackerCache.js";
import { directoryCacheId } from "../src/core/directoryScope.js";
import { workspaceScope } from "../src/core/workspaceModel.js";
import { reviewAddress, writeReviewReply } from "../src/core/taskReviewCache.js";
import { mountWorkspaceReviewEntry } from "../src/core/workspaceReviewEntry.js";
import opened from "../../fixtures/api/v1/tasks.review.open.json";
import pushed from "../../fixtures/api/v1/tasks.review.push.json";

const scope = { deviceId: "pull-device", projectId: "proj-1", workspaceId: "workspace-1" };
const workspaceAddress = { deviceId: scope.deviceId, entityId: "", kind: "workspaces" };
const workspace = { id: scope.workspaceId, project_id: scope.projectId, name: "Work", directories: [
  { id: "dir-api", source_id: "source-api", name: "API", is_git: true, base_branch: "main", branch: "review/1-api" },
  { id: "dir-notes", source_id: "source-notes", name: "Notes", is_git: false },
] };
let entry;
beforeEach(async () => {
  observation.writer = null; observation.contextRead = null;
  await wipeCache(); await wipeUiRecords();
  document.body.innerHTML = '<div id="entry"></div>';
  await writeCached(workspaceAddress, [workspace]);
});
afterEach(() => entry?.dispose());
const enable = (extra = {}) => rememberReviewSupport(scope.deviceId, { reviews: { get: true, snapshot: true, pullRequests: true, open: true, push: true, ...extra } });
const mount = (callRpc = vi.fn()) => { entry = mountWorkspaceReviewEntry(document.querySelector("#entry"), { ...scope, callRpc }); return callRpc; };
const openForm = async () => {
  await vi.waitFor(() => expect(document.querySelector("[data-workspace-review]")).not.toBeNull());
  document.querySelector("[data-workspace-review]").click();
  await vi.waitFor(() => expect(document.querySelector("[data-review-create-form]")?.onsubmit).toBeTypeOf("function"));
};
const type = (selector, value) => { const field = document.querySelector(selector); field.value = value; field.dispatchEvent(new Event("input", { bubbles: true })); };

it("requires the exact Open capability and PR feature, with no render RPC", async () => {
  await enable({ pullRequests: false }); const rpc = mount();
  await vi.waitFor(() => expect(document.querySelector("[data-workspace-review]")).not.toBeNull());
  expect(document.querySelector("#entry").textContent).not.toContain("Open review");
  await enable();
  await vi.waitFor(() => expect(document.querySelector("#entry").textContent).toContain("Open review"));
  expect(rpc).not.toHaveBeenCalled();
});

it("keeps pure-folder workspaces on the legacy snapshot form on PR-capable bridges", async () => {
  await enable();
  await writeCached(workspaceAddress, [{ ...workspace, directories: [workspace.directories[1]] }]);
  await writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [{ id: "legacy-task", number: 8, title: "Review live files" }] });
  const rpc = mount();
  await vi.waitFor(() => expect(document.querySelector("[data-workspace-review]")).not.toBeNull());
  expect(document.querySelector("[data-workspace-review]").textContent).toBe("Create snapshot review");
  document.querySelector("[data-workspace-review]").click();
  await vi.waitFor(() => expect(document.querySelector("[data-review-task]")).not.toBeNull());
  expect(rpc).not.toHaveBeenCalled();
});

it("previews committed sources and restores the exact submitted opening after failure", async () => {
  await enable(); const rpc = mount(vi.fn(async () => { throw new Error("Unavailable"); }));
  await openForm();
  expect([...document.querySelector("#review-create-assignee").options].map((option) => option.value)).not.toContain("new_workspace");
  type("[data-review-title]", "Add CSV export");
  type("[data-review-description]", "Review the exports");
  expect(document.querySelector("[role=dialog]").textContent).toContain("review/<number>-add-csv-export");
  expect(document.querySelector("[role=dialog]").textContent).toContain("Only committed changes are included.");
  expect(document.querySelector("[role=dialog]").textContent).toContain("Live");
  document.querySelector("[data-open-review-submit]").click();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledOnce());
  await vi.waitFor(() => expect(document.querySelector("[role=alert]").textContent).toContain("Unavailable"));
  const request = rpc.mock.calls[0][1];
  expect(request).toMatchObject({ workspace_id: scope.workspaceId, title: "Add CSV export", description: "Review the exports", bases: [{ directory_id: "dir-api", branch: "main" }] });
  const draft = (await readUiRecord(reviewCreateDraftAddress(scope))).value;
  expect(draft.request_id).toBe(request.request_id);
  await entry.dispose(); document.body.innerHTML = '<div id="entry"></div>'; mount(rpc);
  await openForm();
  expect(document.querySelector("[data-review-title]").value).toBe("Add CSV export");
  expect(rpc).toHaveBeenCalledOnce();
  document.querySelector("[data-open-review-submit]").click();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(2));
  expect(rpc.mock.calls[1][1]).toEqual(request);
});

it("keeps a base field's focus and caret through draft readback", async () => {
  await enable(); mount(); await openForm();
  const selector = '[data-review-base="dir-api"]';
  document.querySelector(selector).focus(); type(selector, "release");
  document.querySelector(selector).setSelectionRange(3, 3);
  await observation.writer.flush();
  expect(document.activeElement).toBe(document.querySelector(selector));
  expect(document.activeElement.selectionStart).toBe(3);
});

it("formats reviewed snapshot notes without guessing an ancestry count", () => {
  expect(formatReviewSinceNote({ head: "new", reviewedHead: "old" })).toBe("Changed since your last review");
  expect(formatReviewSinceNote({ head: "new", reviewedHead: "old", rewritten: true })).toBe("History rewritten since your last review");
  expect(formatReviewSinceNote({ head: "same", reviewedHead: "same" })).toBe("");
  expect(formatReviewSinceNote({ head: "new" })).toBe("");
  expect(formatReviewSinceNote({ count: 2 })).toBe("2 commits since your last review");
  expect(formatReviewSinceNote({ count: 1, head: "new", reviewedHead: "old" })).toBe("1 commit since your last review");
  expect(formatReviewSinceNote({ count: 0, head: "same", reviewedHead: "same" })).toBe("");
  expect(pendingReviewText({ sync: { pending_commits: 3 }, binding: {} })).toBe("3 commits not pushed to review");
  expect(pendingReviewText({ sync: { pending_commits: 1 }, binding: {} })).toBe("1 commit not pushed to review");
});

it("shows cached pending commits and publishes the pinned source to its bound destination", async () => {
  await enable();
  const pending = { ...pushed.result.sync[0], revision: 4, pending_commits: 1, working_head: "3".repeat(40), health: "pending" };
  await writeReviewReply({ ...scope, taskId: "task-1" }, { ...opened.result, sync: [pending] }, 1);
  const rpc = mount(vi.fn(async () => pushed.result));
  await vi.waitFor(() => expect(document.querySelector("[data-review-link]")).not.toBeNull());
  expect(document.querySelector("#entry").textContent).toContain("1 commit not pushed to review");
  document.querySelector('[data-review-push="dir-api"]').click();
  await vi.waitFor(() => expect(document.querySelector("[data-push-review-submit]")).not.toBeNull());
  expect(document.querySelector("[role=dialog]").textContent).toContain("build-review-api");
  expect(document.querySelector("[role=dialog]").textContent).toContain("refs/heads/review/1-api");
  document.querySelector("[data-push-review-submit]").click();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledOnce());
  expect(rpc).toHaveBeenCalledWith("tasks.review.push", { task_id: "task-1", expected_version: 1, sources: [{ directory_id: "dir-api", expected_head: pending.working_head, expected_received_head: pending.received_head }] });
  expect((await readCached(reviewAddress({ ...scope, taskId: "task-1" })))).toBeDefined();
});

it("pins the newest cached heads when a commit amendment leaves the pending label unchanged", async () => {
  await enable();
  const pending = { ...pushed.result.sync[0], revision: 4, pending_commits: 1, working_head: "3".repeat(40), health: "pending" };
  const actionScope = { ...scope, taskId: "task-1" };
  await writeReviewReply(actionScope, { ...opened.result, sync: [pending] }, 1);
  mount(vi.fn(async () => { throw new Error("Unavailable"); }));
  await vi.waitFor(() => expect(document.querySelector('[data-review-push="dir-api"]')).not.toBeNull());
  const previousRead = observation.contextRead;
  await writeReviewReply(actionScope, { review: { ...opened.result.review, version: 2 }, sync: [{ ...pending, revision: 5, working_head: "4".repeat(40) }] }, 2);
  expect(observation.contextRead).not.toBe(previousRead);
  // The cache listener registered its paint continuation on this read first.
  await observation.contextRead;
  document.querySelector('[data-review-push="dir-api"]').click();
  await vi.waitFor(() => expect(document.querySelector("[data-push-review-pins]")).not.toBeNull());
  expect(document.querySelector("[data-push-review-pins]").textContent).toContain("Review version 2");
  expect(document.querySelector("[data-push-review-pins]").textContent).toContain("4".repeat(40));
});

it("links the bound PR while on another branch and switches without publishing unrelated HEAD", async () => {
  await enable(); await writeCached(workspaceAddress, [{ ...workspace, directories: [{ ...workspace.directories[0], branch: "other-work" }] }]);
  await writeReviewReply({ ...scope, taskId: "task-1" }, { ...opened.result, sync: pushed.result.sync }, 1);
  const rpc = mount(vi.fn(async () => ({})));
  await vi.waitFor(() => expect(document.querySelector("[data-review-switch]")).not.toBeNull());
  expect(document.querySelector("[data-review-link]")).not.toBeNull();
  expect(document.querySelector('[data-review-push="dir-api"]')).toBeNull();
  document.querySelector("[data-review-switch]").click();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledOnce());
  expect(rpc.mock.calls[0]).toEqual(["git.checkout_ref", { workspace_id: "workspace-1", source_id: "source-api", full_ref: "refs/heads/review/1-api" }]);
});

it("offers switching back for a cached detached checkout without offering Push", async () => {
  await enable();
  await writeReviewReply({ ...scope, taskId: "task-1" }, { ...opened.result, sync: pushed.result.sync }, 1);
  const entityId = directoryCacheId(workspaceScope(scope.workspaceId, "source-api", workspace));
  await writeCached({ deviceId: scope.deviceId, entityId, kind: "refs" }, { current: { kind: "detached", commit: "3".repeat(40) } });
  mount();
  await vi.waitFor(() => expect(document.querySelector("[data-review-switch]")).not.toBeNull());
  expect(document.querySelector('[data-review-push="dir-api"]')).toBeNull();
});

it("suppresses publication when a newer cached workspace summary closes the PR", async () => {
  await enable();
  await writeReviewReply({ ...scope, taskId: "task-1" }, { ...opened.result, sync: pushed.result.sync }, 1);
  await writeCached(workspaceAddress, [{ ...workspace, active_review: { task_id: "task-1", workspace_id: scope.workspaceId, version: 2, status: "closed" } }]);
  mount();
  await vi.waitFor(() => expect(document.querySelector("[data-review-link]")).not.toBeNull());
  expect(document.querySelector(".workspace-review-badge").textContent).toBe("Closed");
  expect(document.querySelector("[data-review-push]")).toBeNull();
});

it("renders changed and rewritten notes against the user's cached reviewed snapshot", async () => {
  await enable();
  const actionScope = { ...scope, taskId: "task-1" };
  await writeTaskRecord(scope.deviceId, scope.projectId, "task-1", taskRecord(opened.result.task, [
    { type: "comment", author: { kind: "user" }, opinion: { snapshot_id: "snapshot-1", verdict: "approve" } },
  ]));
  await writeReviewReply(actionScope, { ...opened.result, sync: pushed.result.sync }, 1);
  mount();
  await vi.waitFor(() => expect(document.querySelector("[data-review-link]")).not.toBeNull());
  expect(document.querySelector("#entry").textContent).not.toContain("since your last review");
  const next = { ...opened.result.review.snapshots[0], id: "snapshot-2", number: 2,
    directories: opened.result.review.snapshots[0].directories.map((directory) => ({ ...directory, head: "4".repeat(40) })) };
  const review = { ...opened.result.review, version: 2, snapshots: [opened.result.review.snapshots[0], next] };
  await writeReviewReply(actionScope, { review }, 2);
  await vi.waitFor(() => expect(document.querySelector("#entry").textContent).toContain("Changed since your last review"));
  await writeReviewReply(actionScope, { review: { ...review, version: 3, snapshots: [review.snapshots[0], { ...next,
    publication: { reason: "received", directories: [{ directory_id: "dir-api", rewritten: true }] } }] } }, 3);
  await vi.waitFor(() => expect(document.querySelector("#entry").textContent).toContain("History rewritten since your last review"));
});

it("renders the bridge's exact count only when it was taken against the user's cached reviewed snapshot", async () => {
  await enable();
  const actionScope = { ...scope, taskId: "task-1" };
  await writeTaskRecord(scope.deviceId, scope.projectId, "task-1", taskRecord(opened.result.task, [
    { type: "comment", author: { kind: "user" }, opinion: { snapshot_id: "snapshot-1", verdict: "approve" } },
  ]));
  const next = { ...opened.result.review.snapshots[0], id: "snapshot-2", number: 2,
    directories: opened.result.review.snapshots[0].directories.map((directory) => ({ ...directory, head: "4".repeat(40) })) };
  const review = { ...opened.result.review, version: 2, snapshots: [opened.result.review.snapshots[0], next] };
  const counted = { ...pushed.result.sync[0], revision: 4, reviewed_snapshot_id: "snapshot-1", commits_since_review: 2,
    snapshot_head: "4".repeat(40), received_head: "5".repeat(40) };
  await writeReviewReply(actionScope, { review, sync: [counted] }, 2);
  mount();
  await vi.waitFor(() => expect(document.querySelector("#entry").textContent).toContain("2 commits since your last review"));
  await writeReviewReply(actionScope, { review: { ...review, version: 3 }, sync: [{ ...counted, revision: 5, reviewed_snapshot_id: "snapshot-0" }] }, 3);
  // Both directories fall back to the qualitative note; neither keeps the stale count.
  await vi.waitFor(() => expect(document.querySelector("#entry").textContent.split("Changed since your last review")).toHaveLength(3));
  expect(document.querySelector("#entry").textContent).not.toContain("commits since your last review");
  await writeReviewReply(actionScope, { review: { ...review, version: 4 }, sync: [{ ...counted, revision: 6, commits_since_review: undefined, rewritten_since_review: true }] }, 4);
  await vi.waitFor(() => expect(document.querySelector("#entry").textContent).toContain("History rewritten since your last review"));
});

it("retains a submitted opening for explicit recovery after its PR arrives in the cache", async () => {
  await enable();
  const rpc = mount(vi.fn(async () => { throw new Error("Lost reply"); }));
  await openForm(); type("[data-review-title]", "Retry review");
  document.querySelector("[data-open-review-submit]").click();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledOnce());
  await vi.waitFor(() => expect(document.querySelector("[role=alert]").textContent).toContain("Lost reply"));
  await entry.dispose(); document.body.innerHTML = '<div id="entry"></div>';
  await writeReviewReply({ ...scope, taskId: "task-1" }, opened.result, 1); mount(rpc);
  await vi.waitFor(() => expect(document.querySelector("[data-workspace-review]").textContent).toContain("Resume opening"));
  expect(document.querySelector("[data-review-link]")).not.toBeNull();
  expect(rpc).toHaveBeenCalledOnce();
  await openForm();
  expect(document.querySelector("[data-review-title]").value).toBe("Retry review");
  document.querySelector("[data-review-form-cancel]").click();
  await enable({ open: false });
  await vi.waitFor(() => expect(document.querySelector("[data-workspace-review]")).toBeNull());
});
