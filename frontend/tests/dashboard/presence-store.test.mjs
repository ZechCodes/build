// presence-store.js — hydrateLastActive() unit coverage.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard/core/bus.js';
import {
  presenceStore,
  SESSION_GAP_MS,
} from '../../src/dashboard/domain/presence-store.js';

test('hydrateLastActive sets lastActiveAt + fires last_active notify', () => {
  const ch = 'p-hydrate';
  const events = [];
  const off = presenceStore.subscribe(e => events.push(e));
  presenceStore.hydrateLastActive(ch, 1_700_000_000_000);
  off();
  assert.equal(presenceStore.get(ch).lastActiveAt, 1_700_000_000_000);
  assert.equal(events.length, 1);
  assert.equal(events[0].kind, 'last_active');
  assert.equal(events[0].channelId, ch);
});

test('hydrateLastActive keeps the larger value (no rewind)', () => {
  const ch = 'p-hydrate-rewind';
  presenceStore.hydrateLastActive(ch, 1_700_000_100_000);
  presenceStore.hydrateLastActive(ch, 1_700_000_000_000);   // older → ignore
  assert.equal(presenceStore.get(ch).lastActiveAt, 1_700_000_100_000);
});

test('hydrateLastActive does NOT flip agentActive', () => {
  const ch = 'p-hydrate-idle';
  assert.equal(presenceStore.get(ch).agentActive, false);
  presenceStore.hydrateLastActive(ch, 1_700_000_000_000);
  assert.equal(presenceStore.get(ch).agentActive, false);
});

test('hydrateLastActive is a no-op when ms is falsy', () => {
  const ch = 'p-hydrate-noop';
  presenceStore.hydrateLastActive(ch, 1_700_000_000_000);
  let fired = 0;
  const off = presenceStore.subscribe(() => fired++);
  presenceStore.hydrateLastActive(ch, 0);
  presenceStore.hydrateLastActive(ch, null);
  presenceStore.hydrateLastActive(ch, undefined);
  off();
  assert.equal(fired, 0);
  assert.equal(presenceStore.get(ch).lastActiveAt, 1_700_000_000_000);
});

test('hydrateSessionStart sets sessionStartAt + fires session_start notify', () => {
  const ch = 'p-session-hydrate';
  const events = [];
  const off = presenceStore.subscribe(e => events.push(e));
  presenceStore.hydrateSessionStart(ch, 1_700_000_000_000);
  off();
  assert.equal(presenceStore.get(ch).sessionStartAt, 1_700_000_000_000);
  assert.equal(events.filter(e => e.kind === 'session_start').length, 1);
});

test('live message starts a session when prior lastActiveAt is more than SESSION_GAP_MS old', () => {
  // Channel was last active 5hrs ago. A fresh message right now opens a
  // new session — the gap exceeds the threshold, so sessionStartAt must
  // advance to the new message's stamp.
  const ch = 'p-session-gap';
  const fiveHoursAgo = 1_700_000_000_000;
  const now = fiveHoursAgo + 5 * 60 * 60 * 1000;
  presenceStore.hydrateLastActive(ch, fiveHoursAgo);
  presenceStore.hydrateSessionStart(ch, fiveHoursAgo);
  bus.emit('message.received', {
    channelId: ch,
    msg: { sender: 'Agent', created_at: new Date(now).toISOString() },
  });
  assert.equal(presenceStore.get(ch).sessionStartAt, now);
  assert.equal(presenceStore.get(ch).lastActiveAt, now);
});

test('live message within an ongoing session does NOT move sessionStartAt', () => {
  // Two messages 10min apart, well below SESSION_GAP_MS. The second
  // should bump lastActiveAt but leave sessionStartAt pinned.
  const ch = 'p-session-stable';
  const sessionStart = 1_700_000_000_000;
  const later = sessionStart + 10 * 60 * 1000;
  presenceStore.hydrateLastActive(ch, sessionStart);
  presenceStore.hydrateSessionStart(ch, sessionStart);
  bus.emit('message.received', {
    channelId: ch,
    msg: { sender: 'Agent', created_at: new Date(later).toISOString() },
  });
  assert.equal(presenceStore.get(ch).sessionStartAt, sessionStart);
  assert.equal(presenceStore.get(ch).lastActiveAt, later);
});

test('first-ever live message seeds sessionStartAt even without a prior gap', () => {
  const ch = 'p-session-firstever';
  const stamp = 1_700_000_000_000;
  bus.emit('message.received', {
    channelId: ch,
    msg: { sender: 'client', created_at: new Date(stamp).toISOString() },
  });
  assert.equal(presenceStore.get(ch).sessionStartAt, stamp);
});

// Reference the import so tooling doesn't flag it as unused.
assert.equal(SESSION_GAP_MS, 4 * 60 * 60 * 1000);
