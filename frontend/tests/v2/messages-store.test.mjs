import { test } from 'node:test';
import assert from 'node:assert/strict';

import { messagesStore } from '../../src/dashboard-v2/domain/messages-store.js';

// Stores are module-level singletons. Use unique channel ids per test to
// avoid cross-test contamination.

test('append adds a message and notifies subscribers', () => {
  const ch = 'ms-append';
  const events = [];
  const off = messagesStore.subscribe(e => events.push(e));

  messagesStore.append(ch, { id: 'm1', content: 'hi' });
  off();

  assert.deepEqual(messagesStore.forChannel(ch), [{ id: 'm1', content: 'hi' }]);
  assert.equal(events.length, 1);
  assert.equal(events[0].kind, 'append');
  assert.equal(events[0].channelId, ch);
  assert.deepEqual(events[0].msg, { id: 'm1', content: 'hi' });
});

test('append dedups by msg.id', () => {
  const ch = 'ms-dedup';
  messagesStore.append(ch, { id: 'x', content: 'first' });
  messagesStore.append(ch, { id: 'x', content: 'dup' });

  const list = messagesStore.forChannel(ch);
  assert.equal(list.length, 1);
  assert.equal(list[0].content, 'first');
});

test('bulk replaces the per-channel list', () => {
  const ch = 'ms-bulk';
  messagesStore.append(ch, { id: 'a', content: 'one' });
  messagesStore.bulk(ch, [
    { id: 'b', content: 'two' },
    { id: 'c', content: 'three' },
  ]);

  const list = messagesStore.forChannel(ch);
  assert.equal(list.length, 2);
  assert.deepEqual(list.map(m => m.id), ['b', 'c']);
});

test('forChannel isolates channels', () => {
  const a = 'ms-iso-a', b = 'ms-iso-b';
  messagesStore.append(a, { id: '1', content: 'a' });
  messagesStore.append(b, { id: '1', content: 'b' });

  assert.equal(messagesStore.forChannel(a)[0].content, 'a');
  assert.equal(messagesStore.forChannel(b)[0].content, 'b');
});

test('forChannel returns empty array for unknown channel', () => {
  assert.deepEqual(messagesStore.forChannel('does-not-exist'), []);
});

test('markRead stamps read_at and notifies once', () => {
  const ch = 'ms-read';
  messagesStore.append(ch, { id: 'r1', content: 'hi' });
  messagesStore.append(ch, { id: 'r2', content: 'ho' });

  const events = [];
  const off = messagesStore.subscribe(e => events.push(e));
  messagesStore.markRead(ch, ['r1']);
  off();

  const list = messagesStore.forChannel(ch);
  assert.ok(list[0].read_at, 'r1 should have read_at');
  assert.equal(list[1].read_at, undefined, 'r2 should not');
  assert.equal(events.length, 1);
  assert.equal(events[0].kind, 'read');
});

test('replaceId swaps temp id to real id and lets delivered match', () => {
  const ch = 'ms-replace';
  messagesStore.append(ch, { id: 'tmp-1', sender: 'client', content: 'hi' });
  messagesStore.replaceId(ch, 'tmp-1', 'real-1');

  const list = messagesStore.forChannel(ch);
  assert.equal(list.length, 1);
  assert.equal(list[0].id, 'real-1');

  messagesStore.markDelivered(ch, 'real-1');
  assert.ok(messagesStore.forChannel(ch)[0].delivered_at, 'delivered_at stamped after replaceId');
});

test('replaceId merges when echo arrived before swap (race)', () => {
  const ch = 'ms-replace-race';
  messagesStore.append(ch, { id: 'tmp-2', sender: 'client', content: 'hi' });
  // Simulate echo landing first: row with real id already present.
  messagesStore.append(ch, { id: 'real-2', sender: 'client', content: 'hi', delivered_at: '2026-01-01T00:00:00Z' });
  messagesStore.replaceId(ch, 'tmp-2', 'real-2');

  const list = messagesStore.forChannel(ch);
  assert.equal(list.length, 1, 'duplicate collapsed');
  assert.equal(list[0].id, 'real-2');
  assert.equal(list[0].delivered_at, '2026-01-01T00:00:00Z');
});

test('replaceId is a no-op when oldId missing or ids match', () => {
  const ch = 'ms-replace-noop';
  messagesStore.append(ch, { id: 'keep', content: 'x' });
  let fired = 0;
  const off = messagesStore.subscribe(() => fired++);
  messagesStore.replaceId(ch, 'missing', 'also-missing');
  messagesStore.replaceId(ch, 'keep', 'keep');
  off();
  assert.equal(fired, 0);
  assert.equal(messagesStore.forChannel(ch)[0].id, 'keep');
});

test('patchInteractionMeta merges into object metadata', () => {
  const ch = 'ms-patch-obj';
  messagesStore.append(ch, {
    id: 'int1',
    metadata: { interaction_id: 'int1', kind: 'question', options: [] },
  });
  const events = [];
  const off = messagesStore.subscribe(e => events.push(e));
  messagesStore.patchInteractionMeta(ch, 'int1', {
    resolved_at: '2026-04-21T00:00:00Z',
    selected_option: 'yes',
  });
  off();
  const m = messagesStore.forChannel(ch)[0];
  assert.equal(typeof m.metadata, 'object');
  assert.equal(m.metadata.interaction_id, 'int1');
  assert.equal(m.metadata.resolved_at, '2026-04-21T00:00:00Z');
  assert.equal(m.metadata.selected_option, 'yes');
  assert.equal(events.length, 1);
  assert.equal(events[0].kind, 'interaction_patch');
});

test('patchInteractionMeta merges into JSON-string metadata + stays stringified', () => {
  const ch = 'ms-patch-json';
  messagesStore.append(ch, {
    id: 'int2',
    metadata: JSON.stringify({ interaction_id: 'int2', kind: 'plan_review' }),
  });
  messagesStore.patchInteractionMeta(ch, 'int2', {
    resolved_at: '2026-04-21T00:00:00Z',
    selected_option: 'approve',
  });
  const m = messagesStore.forChannel(ch)[0];
  assert.equal(typeof m.metadata, 'string');
  const meta = JSON.parse(m.metadata);
  assert.equal(meta.kind, 'plan_review');
  assert.equal(meta.selected_option, 'approve');
  assert.equal(meta.resolved_at, '2026-04-21T00:00:00Z');
});

test('patchInteractionMeta is a no-op for unknown msg ids', () => {
  const ch = 'ms-patch-missing';
  messagesStore.append(ch, { id: 'real', metadata: { kind: 'question' } });
  let fired = 0;
  const off = messagesStore.subscribe(() => fired++);
  messagesStore.patchInteractionMeta(ch, 'nope', { selected_option: 'x' });
  off();
  assert.equal(fired, 0);
  // Original message untouched.
  assert.equal(messagesStore.forChannel(ch)[0].metadata.selected_option, undefined);
});

test('markFailed flags a message and notifies', () => {
  const ch = 'ms-failed';
  messagesStore.append(ch, { id: 'f1', content: 'x' });
  const events = [];
  const off = messagesStore.subscribe(e => events.push(e));
  messagesStore.markFailed(ch, 'f1');
  off();
  assert.equal(messagesStore.forChannel(ch)[0].delivery_failed, true);
  assert.equal(events.length, 1);
  assert.equal(events[0].kind, 'delivery_failed');
});

test('unsubscribe stops delivery (no leak)', () => {
  const ch = 'ms-unsub';
  let fired = 0;
  const off = messagesStore.subscribe(() => fired++);
  messagesStore.append(ch, { id: 'u1', content: 'x' });
  off();
  messagesStore.append(ch, { id: 'u2', content: 'y' });
  assert.equal(fired, 1);
});
