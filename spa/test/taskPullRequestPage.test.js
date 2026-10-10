// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { wipeCache, readCached } from "../src/core/localCache.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { writeReviewReply, reviewAddress } from "../src/core/taskReviewCache.js";
import { mountTaskReviewPage } from "../src/core/taskReviewPage.js";
import fixture from "../../fixtures/api/v1/tasks.review.open.json";
import mergeFixture from "../../fixtures/api/v1/tasks.review.merge.json";
import { deferred, whenDom } from "./reviewLifecycleHarness.js";

vi.mock("../src/core/taskReviewChanges.js", () => ({ mountTaskReviewChanges: (host) => {
  host.textContent = 'Saved changes'; return { dispose() {}, refresh() {} };
} }));
vi.mock("../src/core/taskReviewFiles.js", () => ({ mountTaskReviewFiles: () => ({ dispose() {} }) }));

const scope = { deviceId: 'pr-page', projectId: 'proj-1', taskId: 'task-1' };
const support = { get: true, diff: true, comments: true, pullRequests: true, merge: true, close: true, reopen: true, refresh: true };
let page;
let host;
const review = fixture.result.review;
const snapshot = review.snapshots[0];
const sync = mergeFixture.result.sync;
const nextReview = (version, status, snapshots = review.snapshots) => ({ ...review, version, snapshots,
  pull_request: { ...review.pull_request, status, latest_published_snapshot_id: snapshots.at(-1).id } });
async function mount(callRpc = vi.fn(() => new Promise(() => {})), keepReadingPlace) {
  await rememberReviewSupport(scope.deviceId, { reviews: support });
  await writeReviewReply(scope, { ...fixture.result, sync }, 1);
  page = mountTaskReviewPage(host, { ...scope, callRpc, keepReadingPlace, task: () => ({ id: 'task-1', title: 'Review API' }) });
  await page.ready;
}
beforeEach(async () => {
  await wipeCache(); await wipeUiRecords();
  document.body.innerHTML = '<div id="pr-page"></div>';
  host = document.querySelector('#pr-page');
});
afterEach(() => page?.dispose());

it("keeps historical drafts, anchors and opinions when a new published snapshot resets cached approval", async () => {
  const sent = deferred();
  const callRpc = vi.fn((method, params) => {
    if (method === 'tasks.comment') { sent.resolve(params); return new Promise(() => {}); }
    return new Promise(() => {});
  });
  await mount(callRpc);
  expect(host.textContent).toContain('main ← review/1-api');
  const field = host.querySelector('#task-review-feedback-body');
  field.value = 'Keep this historical thought'; field.dispatchEvent(new Event('input'));
  const anchor = { snapshot_id: snapshot.id, directory_id: 'dir-api', path: 'api.js', side: 'old', line: 2 };
  await page.openAnchor(anchor);
  // This reply binds its draft to the retained snapshot and original anchor.
  await page.reply({ id: 'old-comment', anchor });
  host.querySelector('[data-review-opinion]').value = 'approve';
  host.querySelector('[data-review-opinion]').dispatchEvent(new Event('change'));
  const nextSnapshot = { ...snapshot, id: 'snapshot-2', number: 2 };
  await writeReviewReply(scope, { review: nextReview(2, 'approved') }, 2);
  await page.whenPainted();
  expect(host.querySelector('[data-review-pr-status]').textContent).toBe('Approved');
  await writeReviewReply(scope, { review: nextReview(3, 'open', [snapshot, nextSnapshot]) }, 3);
  await page.whenPainted();
  expect(host.querySelector('[data-review-pr-status]').textContent).toBe('Open');
  expect(host.querySelector('[data-review-snapshot]').value).toBe(snapshot.id);
  expect(host.querySelector('#task-review-feedback-body')).toBe(field);
  expect(field.value).toBe('Keep this historical thought');
  expect(host.querySelector('[data-review-newer-snapshot]').hidden).toBe(false);
  expect(host.querySelector('[data-review-opinion-target]').textContent).toContain('do not change the current review status');
  host.querySelector('[data-review-feedback]').dispatchEvent(new Event('submit', { cancelable: true }));
  expect(await sent.promise).toMatchObject({ opinion: { snapshot_id: snapshot.id, verdict: 'approve' }, anchor, reply_to: 'old-comment' });
  expect(host.querySelector('[data-review-pr-status]').textContent).toBe('Open');
});

it("restores each snapshot's feedback draft when the picker moves between saved snapshots", async () => {
  await mount();
  const field = host.querySelector('#task-review-feedback-body');
  field.value = 'Earlier draft'; field.dispatchEvent(new Event('input'));
  const nextSnapshot = { ...snapshot, id: 'snapshot-2', number: 2 };
  await writeReviewReply(scope, { review: nextReview(2, 'open', [snapshot, nextSnapshot]) }, 2);
  await page.whenPainted();
  const picker = host.querySelector('[data-review-snapshot]');
  picker.value = nextSnapshot.id; picker.dispatchEvent(new Event('change'));
  await whenDom(host, () => host.querySelector('#task-review-feedback-body') !== field);
  const newField = host.querySelector('#task-review-feedback-body');
  newField.value = 'Latest draft'; newField.dispatchEvent(new Event('input'));
  host.querySelector('[data-review-snapshot]').value = snapshot.id;
  host.querySelector('[data-review-snapshot]').dispatchEvent(new Event('change'));
  await whenDom(host, () => host.querySelector('#task-review-feedback-body').value === 'Earlier draft');
  expect(host.querySelector('[data-review-snapshot]').value).toBe(snapshot.id);
});

it("paints the repository's authoritative Close from cache and the same-version later push is a visual no-op", async () => {
  const sent = deferred(); const response = deferred();
  const keepReadingPlace = vi.fn((paint) => paint());
  await mount((method) => {
    if (method === 'tasks.review.close') { sent.resolve(); return response.promise; }
    return new Promise(() => {});
  }, keepReadingPlace);
  const field = host.querySelector('[data-review-close-description]');
  field.value = 'Marked done'; field.dispatchEvent(new Event('input'));
  host.querySelector('[data-review-close]').dispatchEvent(new Event('submit', { cancelable: true }));
  await sent.promise;
  expect(host.querySelector('[data-review-pr-status]').textContent).toBe('Open');
  const answer = { review: { ...nextReview(2, 'closed'), state: 'completed' } };
  response.resolve(answer);
  await whenDom(host, () => host.querySelector('[data-review-pr-status]')?.textContent === 'Closed');
  await whenDom(host, () => host.querySelector('[data-review-reopen]')?.disabled === false);
  expect((await readCached(reviewAddress(scope))).value.review.pull_request.status).toBe('closed');
  const fieldBefore = host.querySelector('#task-review-feedback-body');
  const paints = keepReadingPlace.mock.calls.length;
  await writeReviewReply(scope, answer, 100);
  await page.whenPainted();
  expect(keepReadingPlace).toHaveBeenCalledTimes(paints);
  expect(host.querySelector('#task-review-feedback-body')).toBe(fieldBefore);
  expect(host.querySelector('[data-review-reopen]')).not.toBeNull();
  expect(host.textContent).not.toContain('Merged');
});

it("a stale second client's late Close reply cannot replace newer cached review or observation facts", async () => {
  const sent = deferred(); const response = deferred();
  await mount((method) => {
    if (method === 'tasks.review.close') { sent.resolve(); return response.promise; }
    return new Promise(() => {});
  });
  host.querySelector('[data-review-close-description]').value = 'Old client close';
  host.querySelector('[data-review-close-description]').dispatchEvent(new Event('input'));
  host.querySelector('[data-review-close]').dispatchEvent(new Event('submit', { cancelable: true }));
  await sent.promise;
  const observation = { ...sync[0], revision: 9, error: 'New target observation' };
  await writeReviewReply(scope, { review: nextReview(5, 'changes_requested'), sync: [observation] }, 50);
  await page.whenPainted();
  response.resolve({ review: { ...nextReview(2, 'closed'), state: 'completed' }, sync });
  await whenDom(host, () => !host.querySelector('[data-review-close] button').disabled);
  expect(host.querySelector('[data-review-pr-status]').textContent).toBe('Changes requested');
  expect((await readCached(reviewAddress(scope))).value).toMatchObject({ review: { version: 5 }, sync: [observation, ...sync.slice(1)] });
  expect(host.querySelector('[data-review-reopen]')).toBeNull();
});
