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

test('unsubscribe stops delivery (no leak)', () => {
  const ch = 'ms-unsub';
  let fired = 0;
  const off = messagesStore.subscribe(() => fired++);
  messagesStore.append(ch, { id: 'u1', content: 'x' });
  off();
  messagesStore.append(ch, { id: 'u2', content: 'y' });
  assert.equal(fired, 1);
});
