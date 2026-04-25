import { test } from 'node:test';
import assert from 'node:assert/strict';

import { unreadStore } from '../../src/dashboard/domain/unread-store.js';

test('increment raises the count', () => {
  const ch = 'u-inc';
  unreadStore.increment(ch);
  unreadStore.increment(ch);
  unreadStore.increment(ch);
  assert.equal(unreadStore.get(ch).count, 3);
  assert.equal(unreadStore.get(ch).hasInteraction, false);
});

test('hasInteraction latches until markRead', () => {
  const ch = 'u-interact';
  unreadStore.increment(ch, false);
  unreadStore.increment(ch, true);   // latches
  unreadStore.increment(ch, false);  // stays true
  assert.equal(unreadStore.get(ch).hasInteraction, true);

  unreadStore.markRead(ch);
  assert.equal(unreadStore.get(ch).hasInteraction, false);
});

test('markRead zeros the count and stamps lastSeen', () => {
  const ch = 'u-mark';
  unreadStore.increment(ch);
  unreadStore.increment(ch);
  unreadStore.markRead(ch, '2026-04-20T00:00:00Z');

  const slot = unreadStore.get(ch);
  assert.equal(slot.count, 0);
  assert.equal(slot.lastSeen, '2026-04-20T00:00:00Z');
});

test('get returns neutral slot for unknown channel', () => {
  const slot = unreadStore.get('u-nope');
  assert.deepEqual(slot, { count: 0, hasInteraction: false, lastSeen: null });
});

test('aggregate sums across channels', () => {
  // Use isolated channels so other tests do not pollute the count.
  const a = 'u-agg-a', b = 'u-agg-b', c = 'u-agg-c';
  unreadStore.markRead(a);
  unreadStore.markRead(b);
  unreadStore.markRead(c);

  const before = unreadStore.aggregate();

  unreadStore.increment(a);
  unreadStore.increment(a);
  unreadStore.increment(b, true);
  // c: 0

  const after = unreadStore.aggregate();
  assert.equal(after.total - before.total, 3);
  assert.equal(after.anyInteraction, true);

  unreadStore.markRead(a);
  unreadStore.markRead(b);
});

test('hydrate sets count + hasInteraction directly', () => {
  const ch = 'u-hydrate';
  const events = [];
  const off = unreadStore.subscribe(e => events.push(e));
  unreadStore.hydrate(ch, 5, true);
  off();
  assert.equal(unreadStore.get(ch).count, 5);
  assert.equal(unreadStore.get(ch).hasInteraction, true);
  assert.equal(events.length, 1);
  assert.equal(events[0].kind, 'hydrate');
  assert.equal(events[0].channelId, ch);
});

test('hydrate with 0 + false clears the slot', () => {
  const ch = 'u-hydrate-zero';
  unreadStore.hydrate(ch, 3, true);
  unreadStore.hydrate(ch, 0, false);
  assert.equal(unreadStore.get(ch).count, 0);
  assert.equal(unreadStore.get(ch).hasInteraction, false);
});

test('notify fires on increment and markRead', () => {
  const ch = 'u-notify';
  const events = [];
  const off = unreadStore.subscribe(e => events.push(e));
  unreadStore.increment(ch);
  unreadStore.markRead(ch);
  off();
  assert.deepEqual(events.map(e => e.kind), ['increment', 'read']);
});
