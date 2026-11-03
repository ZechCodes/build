// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import { wipeCache } from "../src/core/localCache.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { writeReviewRecord } from "../src/core/taskReviewCache.js";
import { mountTaskReviewPage } from "../src/core/taskReviewPage.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";
import prFixture from "../../fixtures/api/v1/tasks.review.open.json";
import { painted } from "./waits.js";

const panes = vi.hoisted(() => ({ changes: vi.fn(), files: vi.fn(), refresh: vi.fn() }));
vi.mock("../src/core/taskReviewChanges.js", () => ({ mountTaskReviewChanges: (host, options) => {
  panes.changes(options); host.textContent = `Changes ${options.directory.id}`;
  return { dispose() {}, refresh: panes.refresh };
} }));
vi.mock("../src/core/taskReviewFiles.js", () => ({ mountTaskReviewFiles: (host, options) => {
  panes.files(options); host.textContent = `Files ${options.directory.id} ${options.path || ""}`;
  return { dispose() {}, refresh: panes.refresh, open: vi.fn() };
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
  await painted(() => document.querySelector('[data-review-snapshot]'));
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
  await painted(() => document.querySelector('[data-review-read-only]')?.textContent.includes("Approved"));
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
  expect([...viewGroup.querySelectorAll('button')].map((button) => button.textContent.trim())).toEqual(["Changes", "Files", "Comments"]);
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
  await painted(() => document.querySelector('[data-review-open-files]'));
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

it("shows the pending tray and verdict bar and places lifecycle controls in More", async () => {
  await mount(review, { ...support, comments: true });
  expect(document.querySelector('[data-review-pending]')).not.toBeNull();
  expect(document.querySelector('[data-review-more] [data-review-save]')).not.toBeNull();
  expect(document.querySelector('[data-review-more] [data-review-complete]')).not.toBeNull();
});

it("restores a URL selection offline and preserves path and line when changing revision", async () => {
  const older = { ...review.snapshots[0], id: 'older', number: 1 };
  const newer = { ...review.snapshots[0], id: 'newer', number: 2 };
  const saved = { ...review, snapshots: [older, newer] };
  await rememberReviewSupport(scope.deviceId, { reviews: support });
  await writeReviewRecord(scope, saved, 1);
  const navigate = vi.fn();
  page = mountTaskReviewPage(document.querySelector('#review'), {
    ...scope, callRpc: vi.fn(() => new Promise(() => {})), markReviewRoute: navigate,
    reviewRoute: { revision: 1, mode: 'all', directoryId: 'dir-api', path: 'cached.js', side: 'new', line: 42 },
  });
  await page.ready;
  expect(panes.changes.mock.calls.at(-1)[0]).toMatchObject({ snapshot: { id: 'older' }, path: 'cached.js', anchor: { path: 'cached.js', side: 'new', line: 42 } });
  const select = document.querySelector('[data-review-snapshot]');
  select.value = 'newer'; select.dispatchEvent(new Event('change'));
  expect(navigate.mock.calls.at(-1)[0].review).toMatchObject({ revision: 2, path: 'cached.js', side: 'new', line: 42 });
  expect(panes.changes.mock.calls.at(-1)[0].snapshot.id).toBe('newer');
});

it("refreshes destinations after saving a snapshot whose mutation omits them", async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: { ...support, act: true } });
  await writeReviewRecord(scope, review, 1);
  const next = { ...review, version: 2, snapshots: [...review.snapshots, { ...review.snapshots[0], id: "new-snapshot", number: 2 }] };
  const callRpc = vi.fn(async (method) => ({ review: method === "tasks.review.snapshot" ? { ...next, destinations: [] } : {
    ...next, destinations: [{ snapshot_id: "new-snapshot", directory_id: "dir-api", source_path: "/sources/api",
      branches: ["main"], remotes: [{ name: "origin", branches: ["main"] }], live_head: review.snapshots[0].directories[0].head }],
  } }));
  // The page tells the task once the save, its re-read and the move to the new snapshot are all done.
  let saved;
  const settled = new Promise((resolve) => { saved = resolve; });
  page = mountTaskReviewPage(document.querySelector("#review"), {
    ...scope, callRpc, workspaces: () => [], task: () => ({ id: scope.taskId }), onTaskChanged: () => saved(),
  });
  await painted(() => document.querySelector('[data-review-save]'));
  document.querySelector('[data-review-save]').dispatchEvent(new Event("submit", { cancelable: true }));
  await settled;
  await painted(() => document.querySelector('[data-review-source="dir-api"]'));
  const methods = callRpc.mock.calls.map(([method]) => method);
  expect(methods.lastIndexOf("tasks.review.get")).toBeGreaterThan(methods.indexOf("tasks.review.snapshot"));
});

it('falls back to an all-changes URL when a linked comparison is unsupported', async () => {
  await rememberReviewSupport(scope.deviceId, { reviews: support });
  await writeReviewRecord(scope, review, 1);
  const navigate = vi.fn();
  page = mountTaskReviewPage(document.querySelector('#review'), {
    ...scope, callRpc: vi.fn(() => new Promise(() => {})), markReviewRoute: navigate,
    reviewRoute: { revision: 1, mode: 'commit', commit: 'a'.repeat(40), directoryId: 'dir-api', path: 'a.js' },
  });
  await page.ready;
  expect(document.querySelector('[data-review-range="all"]').getAttribute('aria-pressed')).toBe('true');
  expect(navigate.mock.calls.at(-1)[0].review).toMatchObject({ mode: 'all', path: 'a.js' });
});

it("keeps automatic refresh metadata-only while an explicit Refresh can retry the pane", async () => {
  await mount();
  page.refresh();
  expect(panes.refresh).not.toHaveBeenCalled();
  document.querySelector('[data-review-refresh]').click();
  expect(panes.refresh).toHaveBeenCalledTimes(1);
});


it("enables line composition in both panes and disables verdicts on older revisions", async () => {
  const first = { ...review.snapshots[0], id: 'first', number: 1 };
  const second = { ...first, id: 'second', number: 2 };
  await mount({ ...review, snapshots: [first, second] }, { ...support, comments: true });
  expect(panes.changes.mock.calls.at(-1)[0].onComment).toBeTypeOf('function');
  expect([...document.querySelectorAll('[data-review-verdict]')].every((button) => !button.disabled)).toBe(true);
  const select = document.querySelector('[data-review-snapshot]');
  select.value = 'first'; select.dispatchEvent(new Event('change'));
  await painted(() => document.querySelector('[data-review-draft-revision]')?.textContent.includes('earlier revision'));
  expect([...document.querySelectorAll('[data-review-verdict]')].every((button) => button.disabled)).toBe(true);
  document.querySelector('[data-review-view="files"]').click();
  expect(panes.files.mock.calls.at(-1)[0].onComment).toBeTypeOf('function');
});
