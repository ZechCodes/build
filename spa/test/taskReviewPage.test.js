// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import { wipeCache } from "../src/core/localCache.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { writeReviewRecord } from "../src/core/taskReviewCache.js";
import { mountTaskReviewPage } from "../src/core/taskReviewPage.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";

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
async function mount(saved = review) {
  await rememberReviewSupport(scope.deviceId, { reviews: support });
  if (saved) await writeReviewRecord(scope, saved, 1);
  page = mountTaskReviewPage(document.querySelector("#review"), {
    ...scope, callRpc: vi.fn(() => new Promise(() => {})),
    workspaces: () => [], task: () => ({ id: scope.taskId }), onTaskChanged: vi.fn(),
  });
  await vi.waitFor(() => expect(document.querySelector('[data-review-snapshot]')).not.toBeNull());
}

it("renders every saved directory and switches Changes to embedded Files in the same directory", async () => {
  await mount();
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
