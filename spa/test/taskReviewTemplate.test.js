// @vitest-environment jsdom
import { expect, it } from 'vitest';
import { taskPageParts, isReviewTask, taskPageFrame } from '../src/core/trackerTaskRender.js';
import { reviewRailHtml, reviewBaseline } from '../src/core/taskReviewRender.js';

const task = { id: 't', title: 'Review caching', state: 'open', status: 'in_review', labels: [], attachments: [] };
const context = { rows: [], links: [], columns: [], draft: '', labelsDraft: '', busy: false, sending: false, reviewSnapshots: [] };
it('chooses the review template from a task summary or a cached review', () => {
  expect(isReviewTask(task, {})).toBe(false);
  expect(isReviewTask({ ...task, review_summary: { status: 'open' } }, {})).toBe(true);
  expect(isReviewTask(task, { review: { snapshots: [] } })).toBe(true);
  expect(taskPageFrame(true)).toContain('review-task-template');
  const parts = taskPageParts({ ...task, review_summary: { status: 'open' } }, context);
  expect(parts.main.map(part => part.name)).toEqual(['review', 'timeline']);
  expect(parts.main.find(part => part.name === 'timeline').html).toContain('task-review-comments');
});
it('shows only All changes without independent commit capabilities', () => {
  const html = reviewRailHtml({ support: {}, selection: { mode: 'all' }, commits: { commits: [{ oid: 'a', subject: 'Fix' }] }, baseline: { id: 'r1' } });
  expect(html).toContain('All changes');
  expect(html).not.toContain('Since my review');
  expect(html).not.toContain('data-review-commit=');
});
it('shows cached commit metadata and a baseline only from the user last opinion', () => {
  const snapshots = [{ id: 'r1', number: 1 }, { id: 'r2', number: 2 }];
  const rows = [
    { actor: { kind: 'user' }, opinion: { snapshot_id: 'r1' } },
    { actor: { kind: 'agent' }, opinion: { snapshot_id: 'r2' } },
  ];
  expect(reviewBaseline({ snapshots }, rows)).toEqual(snapshots[0]);
  const args = { support: { commits: true, commit: true, since: true }, selection: { mode: 'all' }, commits: { commits: [{ oid: 'a'.repeat(40), subject: '<Fix>', author: { name: 'Ada' }, additions: 2, deletions: 1 }] } };
  expect(reviewRailHtml(args)).not.toContain('Since my review');
  const html = reviewRailHtml({ ...args, baseline: snapshots[0] });
  expect(html).toContain('Since my review');
  expect(html).toContain('&lt;Fix&gt;');
  expect(html).toContain('Ada');
  expect(html).toContain('+2');
});

it('uses only earlier opinion revisions as a since baseline and scopes cached counts to that head', async () => {
  const { reviewComparisonBaseline, reviewSinceCount } = await import('../src/core/taskReviewRender.js');
  const snapshots = [{ id: 'r1', number: 1 }, { id: 'r2', number: 2 }];
  const rows = [{ actor: { kind: 'user' }, opinion: { snapshot_id: 'r2' } }];
  expect(reviewComparisonBaseline({ snapshots }, rows, snapshots[0])).toBeNull();
  expect(reviewComparisonBaseline({ snapshots }, rows, snapshots[1])).toBeNull();
  rows[0].opinion.snapshot_id = 'r1';
  expect(reviewComparisonBaseline({ snapshots }, rows, snapshots[1])).toEqual(snapshots[0]);
  const directory = { id: 'dir', head: 'head2' };
  const record = { sync: [{ directory_id: 'dir', snapshot_head: 'head2', reviewed_snapshot_id: 'r1', commits_since_review: 3 }] };
  expect(reviewSinceCount(record, snapshots[0], directory)).toBe(3);
  expect(reviewSinceCount(record, snapshots[1], directory)).toBeNull();
  expect(reviewSinceCount(record, snapshots[0], { ...directory, head: 'old' })).toBeNull();
});
