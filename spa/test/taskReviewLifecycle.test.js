// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { mountTaskReviewLifecycle } from "../src/core/taskReviewLifecycle.js";
import fixture from "../../fixtures/api/v1/tasks.review.open.json";
import mergeFixture from "../../fixtures/api/v1/tasks.review.merge.json";
import { whenDom, deferred } from "./reviewLifecycleHarness.js";

const scope = { deviceId: "lifecycle-device", projectId: "proj-1", taskId: "task-1" };
const support = { pullRequests: true, close: true, reopen: true, refresh: true };
const review = fixture.result.review;
let controls;
let host;
const mount = (over = {}) => {
  controls = mountTaskReviewLifecycle(host, { ...scope, review, support, repository: { mutate: vi.fn() }, ...over });
  return controls.ready;
};
beforeEach(async () => {
  await wipeCache(); await wipeUiRecords();
  document.body.innerHTML = '<div id="lifecycle"></div>';
  host = document.querySelector('#lifecycle');
});
afterEach(() => controls?.dispose());

it("gates lifecycle by the PR feature and exact verbs without offering legacy mutations", async () => {
  await mount({ support: { close: true, reopen: true, refresh: true } });
  expect(host.querySelector('[data-review-close]')).toBeNull();
  expect(host.querySelector('[data-review-reopen]')).toBeNull();
  expect(host.querySelector('[data-review-repair]')).toBeNull();
  expect(host.textContent).toContain('Read-only');
  controls.dispose();
  await mount({ support: { ...support, close: false } });
  expect(host.querySelector('[data-review-close]')).toBeNull();
  expect(host.querySelector('[data-review-advanced] summary').textContent).toBe('Advanced');
  expect(host.querySelector('[data-review-repair]').textContent).toBe('Refresh published review');
  expect(host.textContent).not.toContain('Update review');
});

it("submits Close once with the cached version, keeps its stale draft and never paints a returned review", async () => {
  const sent = deferred();
  const response = deferred();
  const repository = { mutate: vi.fn((verb, params) => { sent.resolve([verb, params]); return response.promise; }) };
  await mount({ repository });
  const field = host.querySelector('[data-review-close-description]');
  field.value = '  Superseded by another approach  ';
  field.dispatchEvent(new Event('input'));
  host.querySelector('[data-review-close]').dispatchEvent(new Event('submit', { cancelable: true }));
  host.querySelector('[data-review-close]').dispatchEvent(new Event('submit', { cancelable: true }));
  expect(await sent.promise).toEqual(['close', { expected_version: review.version, description: 'Superseded by another approach' }]);
  expect(repository.mutate).toHaveBeenCalledTimes(1);
  response.reject(Object.assign(new Error('Review changed; refresh and try again'), { code: 'stale_version' }));
  await whenDom(host, () => host.querySelector('[data-review-lifecycle-error]')?.textContent.includes('Review changed'));
  controls.dispose();
  await mount({ repository: { mutate: vi.fn(async () => ({ review: { ...review, pull_request: { status: 'closed' } } })) } });
  expect(host.querySelector('[data-review-close-description]').value).toBe('  Superseded by another approach  ');
  host.querySelector('[data-review-close]').dispatchEvent(new Event('submit', { cancelable: true }));
  await whenDom(host, () => !host.querySelector('[data-review-close] button').disabled);
  expect(host.querySelector('[data-review-reopen]')).toBeNull();
});

it("offers Reopen only for cached Closed PRs and uses the current version", async () => {
  const sent = deferred();
  const repository = { mutate: vi.fn((...args) => { sent.resolve(args); return new Promise(() => {}); }) };
  await mount({ repository });
  expect(host.querySelector('[data-review-reopen]')).toBeNull();
  controls.update({ ...review, version: 8, state: 'completed', pull_request: { ...review.pull_request, status: 'closed' } });
  host.querySelector('[data-review-reopen]').click();
  expect(await sent.promise).toEqual(['reopen', { expected_version: 8 }]);
  controls.update({ ...review, version: 9, state: 'completed', pull_request: { ...review.pull_request, status: 'merged' } });
  expect(host.querySelector('[data-review-reopen]')).toBeNull();
  expect(host.querySelector('[data-review-close]')).toBeNull();
});

it("Refresh published review is an explicit repair, never a reconnect or mount action", async () => {
  const sent = deferred();
  const repository = { mutate: vi.fn((...args) => { sent.resolve(args); return new Promise(() => {}); }) };
  await mount({ repository });
  expect(repository.mutate).not.toHaveBeenCalled();
  host.querySelector('[data-review-repair]').click();
  expect(await sent.promise).toEqual(['refresh', { expected_version: review.version }]);
});

it("offers reclaim only for a settled merge and follows cached locks, including a stale-client locked refusal", async () => {
  const merged = mergeFixture.result.review;
  const workspace = { workspace_id: review.workspace_id, locked: true };
  const address = { deviceId: scope.deviceId, entityId: '', kind: 'workspaces' };
  await writeCached(address, [workspace]);
  const callRpc = vi.fn(async () => { throw Object.assign(new Error('Workspace is locked. Unlock it to delete it.'), { code: 'locked' }); });
  await mount({ review: merged, record: mergeFixture.result, callRpc });
  const button = host.querySelector('[data-review-reclaim]');
  expect(button.disabled).toBe(true);
  expect(button.title).toBe('Unlock the workspace to delete it');
  expect(host.textContent).toContain('Merged · Workspace locked');
  await writeCached(address, [{ ...workspace, locked: false }]);
  await whenDom(host, () => !button.disabled);
  expect(host.querySelector('[data-review-reclaim]')).toBe(button);
  button.click();
  await whenDom(host, () => host.querySelector('[data-review-delete-error]')?.textContent.includes('Workspace is locked'));
  expect(callRpc).toHaveBeenCalledWith('workspace.reclaim', { workspace_id: review.workspace_id });
  expect(button.disabled).toBe(false);
  controls.update(merged, { review: merged, merge_intents: [{ state: 'failed' }] });
  expect(host.querySelector('[data-review-reclaim]')).toBeNull();
});

it("a cached settled merge remains read-only without the PR feature", async () => {
  await writeCached({ deviceId: scope.deviceId, entityId: '', kind: 'workspaces' }, [{ workspace_id: review.workspace_id, locked: false }]);
  await mount({ review: mergeFixture.result.review, record: mergeFixture.result, support: { close: true, reopen: true, refresh: true, merge: true } });
  expect(host.querySelector('[data-review-reclaim]')).toBeNull();
  expect(host.textContent).toContain('Read-only');
});
