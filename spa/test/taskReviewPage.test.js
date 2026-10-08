// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import { wipeCache } from "../src/core/localCache.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { writeReviewRecord } from "../src/core/taskReviewCache.js";
import { mountTaskReviewPage } from "../src/core/taskReviewPage.js";
import { writeTaskRecord } from "../src/core/trackerCache.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";
import prFixture from "../../fixtures/api/v1/tasks.review.open.json";

const panes = vi.hoisted(() => ({ changes: vi.fn(), files: vi.fn() }));
vi.mock("../src/core/taskReviewChanges.js", () => ({ mountTaskReviewChanges: (host, options) => {
  panes.changes(options); host.textContent = `Changes ${options.directory.id}`;
  return { dispose() {}, refresh() {} };
} }));
vi.mock("../src/core/taskReviewFiles.js", () => ({ mountTaskReviewFiles: (host, options) => {
  panes.files(options); host.textContent = `Files ${options.directory.id} ${options.path || ""}`;
  return { dispose() {}, refresh() {}, open: vi.fn() };
} }));

const scope = { deviceId: "review-page", projectId: "proj-1", taskId: "task-1" };
const support = { get: true, snapshot: true, diff: true, complete: true, comments: false };
const review = fixture.result.review;
let page;
beforeEach(async () => {
  page?.dispose(); vi.clearAllMocks(); await wipeCache(); await wipeUiRecords();
  document.body.innerHTML = '<div id="review"></div>';
});
async function mount(saved = review, cachedSupport = support) {
  await rememberReviewSupport(scope.deviceId, { reviews: cachedSupport });
  if (saved) await writeReviewRecord(scope, saved, 1);
  page = mountTaskReviewPage(document.querySelector("#review"), {
    ...scope, callRpc: vi.fn(() => new Promise(() => {})),
    workspaces: () => [], task: () => ({ id: scope.taskId }), onTaskChanged: vi.fn(),
  });
  await vi.waitFor(() => expect(document.querySelector('[data-review-snapshot]')).not.toBeNull());
}

it("routes cached PRs to read-only controls and suppresses legacy git actions", async () => {
  const cachedSupport = { ...support, act: true, pullRequests: true, open: true, push: true };
  await mount(prFixture.result.review, cachedSupport);
  expect(document.querySelector('[data-review-read-only]').textContent).toContain("Open");
  expect(document.querySelector('[data-review-save]')).toBeNull();
  expect(document.querySelector('[data-review-complete]')).toBeNull();
  expect(document.querySelector('[data-review-act]')).toBeNull();
  expect(panes.changes).toHaveBeenCalled();

  await writeReviewRecord(scope, { ...prFixture.result.review, version: 2,
    pull_request: { ...prFixture.result.review.pull_request, status: "approved" } }, 2);
  await vi.waitFor(() => expect(document.querySelector('[data-review-read-only]').textContent).toContain("Approved"));
  expect(document.querySelector('[data-review-act]')).toBeNull();
});

it("keeps legacy snapshot, completion and git action controls on PR-capable devices", async () => {
  await mount(review, { ...support, act: true, pullRequests: true, open: true, push: true });
  expect(document.querySelector('[data-review-save]')).not.toBeNull();
  expect(document.querySelector('[data-review-complete]')).not.toBeNull();
  expect(document.querySelector('[data-review-act]')).not.toBeNull();
  expect(document.querySelector('[data-review-read-only]')).toBeNull();
});

it("renders every saved directory and switches Changes to embedded Files in the same directory", async () => {
  await mount();
  const viewGroup = document.querySelector('[role="group"][aria-label="Directory view"]');
  expect(viewGroup).not.toBeNull();
  expect([...viewGroup.querySelectorAll('button')].map((button) => button.textContent)).toEqual(["Changes", "Files"]);
  expect([...document.querySelectorAll('[data-directory]')].map((node) => node.textContent)).toEqual(["API", "Notes", "Removed source"]);
  expect(document.querySelector("#review").textContent).toContain("2 uncommitted files not in this review");
  panes.changes.mock.calls[0][0].onOpenFile("unchanged.txt");
  await vi.waitFor(() => expect(panes.files.mock.calls.at(-1)[0].directory.id).toBe("dir-api"));
  expect(panes.files.mock.calls.at(-1)[0]).toMatchObject({ directory: { id: "dir-api" }, path: "unchanged.txt" });
});

it("defaults non-Git directories to Files and says the files are live", async () => {
  await mount();
  document.querySelector('[data-directory="dir-notes"]').click();
  await vi.waitFor(() => expect(panes.files.mock.calls.at(-1)[0].directory.id).toBe("dir-notes"));
  expect(document.querySelector("#review").textContent).toContain("Not a Git repository");
  expect(document.querySelector("#review").textContent).toContain("Live files — not saved with this review");
  document.querySelector('[data-review-view="changes"]').click();
  await vi.waitFor(() => expect(document.querySelector('[data-review-open-files]')).not.toBeNull());
});

it("keeps old snapshot anchors and reports unavailable context after history replacement", async () => {
  const old = { ...review.snapshots[0], id: "old", number: 1 };
  const latest = { ...review.snapshots[0], id: "new", number: 2 };
  await mount({ ...review, version: 2, snapshots: [old, latest] });
  await page.openAnchor({ snapshot_id: "old", directory_id: "dir-api", path: "old.txt", side: "old", line: 2 });
  await vi.waitFor(() => expect(document.querySelector('[data-review-snapshot]').value).toBe("old"));
  await page.openAnchor({ snapshot_id: "gone", directory_id: "dir-api", path: "old.txt", side: "old", line: 2 });
  expect(document.querySelector("#review").textContent).toContain("Original review context unavailable");
});

it("does not call review verbs or draw actions without cached support", async () => {
  const callRpc = vi.fn();
  page = mountTaskReviewPage(document.querySelector("#review"), { ...scope, callRpc });
  await new Promise((resolve) => setTimeout(resolve, 40));
  expect(callRpc).not.toHaveBeenCalled();
  expect(document.querySelector('[data-review-update]')).toBeNull();
});

it("uses the timeline's cached roster names in a reply draft and updates them when the feed moves", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: { ...support, comments: true } });
  await writeReviewRecord(scope, review, 1);
  const parent = { type: "comment", id: "tc-parent", author: { kind: "agent", agent_id: "agent-one" }, body: "Parent excerpt" };
  await writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, { task: { id: scope.taskId }, timeline: [parent] });
  const projectKey = `${scope.deviceId}/${scope.projectId}`;
  const feed = { projects: [{ projectKey, name: "Build" }],
    workspaces: [{ projectKey, id: "ws", name: "Workspace" }],
    items: [{ projectKey, entity_id: "ws", agents: [{ id: "agent-one", name: "Ada" }] }] };
  const callRpc = vi.fn(() => new Promise(() => {}));
  page = mountTaskReviewPage(document.querySelector("#review"), {
    ...scope, projectKey, feed: () => feed, callRpc, task: () => ({ id: scope.taskId }),
  });
  await vi.waitFor(() => expect(document.querySelector('[data-review-feedback]')).not.toBeNull());
  await page.reply({ ...parent, id: parent.id });
  await vi.waitFor(() => expect(document.querySelector('[data-review-target]').textContent).toBe("Replying to Workspace · Ada · Parent excerpt"));
  const field = document.querySelector('#task-review-feedback-body');
  feed.items[0].agents[0].name = "Grace";
  page.feedMoved();
  expect(document.querySelector('[data-review-target]').textContent).toBe("Replying to Workspace · Grace · Parent excerpt");
  expect(document.querySelector('#task-review-feedback-body')).toBe(field);
  expect(callRpc.mock.calls.every(([method]) => method === "tasks.review.get")).toBe(true);
});

it("refreshes destinations after saving a snapshot whose mutation omits them", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: { ...support, act: true } });
  await writeReviewRecord(scope, review, 1);
  const next = { ...review, version: 2, snapshots: [...review.snapshots, { ...review.snapshots[0], id: "new-snapshot", number: 2 }] };
  const callRpc = vi.fn(async (method) => ({ review: method === "tasks.review.snapshot" ? { ...next, destinations: [] } : {
    ...next, destinations: [{ snapshot_id: "new-snapshot", directory_id: "dir-api", source_path: "/sources/api",
      branches: ["main"], remotes: [{ name: "origin", branches: ["main"] }], live_head: review.snapshots[0].directories[0].head }],
  } }));
  page = mountTaskReviewPage(document.querySelector("#review"), { ...scope, callRpc, workspaces: () => [], task: () => ({ id: scope.taskId }) });
  await vi.waitFor(() => expect(document.querySelector('[data-review-save]')).not.toBeNull());
  document.querySelector('[data-review-save]').dispatchEvent(new Event("submit", { cancelable: true }));
  await vi.waitFor(() => expect(document.querySelector('[data-review-source="dir-api"]')).not.toBeNull());
  const methods = callRpc.mock.calls.map(([method]) => method);
  expect(methods.lastIndexOf("tasks.review.get")).toBeGreaterThan(methods.indexOf("tasks.review.snapshot"));
});
