import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard/core/bus.js';
import { channelsStore } from '../../src/dashboard/domain/channels-store.js';
import { presenceStore } from '../../src/dashboard/domain/presence-store.js';
import { unreadStore } from '../../src/dashboard/domain/unread-store.js';
import { buildAttentionList } from '../../src/dashboard/shell/channel-panel.js';

const NOW = 1_800_000_000_000;
const HOUR = 60 * 60 * 1000;

// Pin Date.now() up front so live-stamping paths (setAgentActive,
// the message.received fallback when created_at is missing) align
// with the synthetic NOW the tests pass into buildAttentionList.
Date.now = () => NOW;

beforeEach(() => {
  // The stores are module-level singletons — without a reset, state
  // bleeds across tests and the 10-cap behavior depends on which
  // tests ran before. Each `__resetForTests__` is a single-line
  // wrapper around `Map.clear()`.
  channelsStore.__resetForTests__();
  presenceStore.__resetForTests__();
  unreadStore.__resetForTests__();
});

function seedChannel(id, ageHours) {
  bus.emit('channel.upserted', { deviceId: 'dev-test', channel: { id, name: id } });
  presenceStore.hydrateLastActive(id, NOW - ageHours * HOUR);
}

const ids = items => items.map(i => i.ch.id);

test('primary group: 4hr entries sorted oldest first', () => {
  seedChannel('fresh', 0.5);
  seedChannel('mid', 1.5);
  seedChannel('old', 3);
  const { primary } = buildAttentionList(NOW);
  assert.deepEqual(ids(primary), ['old', 'mid', 'fresh']);
});

test('fallback group: 4–72hr entries sorted newest first', () => {
  seedChannel('near', 5);
  seedChannel('far', 60);
  seedChannel('mid', 30);
  const { fallback } = buildAttentionList(NOW);
  assert.deepEqual(ids(fallback), ['near', 'mid', 'far']);
});

test('10-cap: 7 primary + 5 fallback candidates → 7 primary + 3 fallback', () => {
  for (let i = 0; i < 7; i++) seedChannel(`p${i}`, 0.1 + i * 0.1);
  for (let i = 0; i < 5; i++) seedChannel(`f${i}`, 5 + i);
  const { primary, fallback } = buildAttentionList(NOW);
  assert.equal(primary.length, 7);
  assert.equal(fallback.length, 3);
  // Fallback is newest-first; the three closest-to-now win their slots.
  assert.deepEqual(ids(fallback), ['f0', 'f1', 'f2']);
});

test('10-cap: 12 primary entries → keep the 10 newest (oldest 2 dropped)', () => {
  for (let i = 0; i < 12; i++) seedChannel(`p${i}`, 0.1 + i * 0.1);
  const { primary, fallback } = buildAttentionList(NOW);
  assert.equal(primary.length, 10);
  assert.equal(fallback.length, 0);
  // Oldest two (p11=1.2hr, p10=1.1hr) are dropped; sorted oldest-first
  // the remaining list runs from p9 (1.0hr) down to p0 (0.1hr).
  assert.deepEqual(ids(primary), ['p9', 'p8', 'p7', 'p6', 'p5', 'p4', 'p3', 'p2', 'p1', 'p0']);
});

test('window boundaries: exactly-4hr lands in fallback, exactly-72hr is excluded', () => {
  bus.emit('channel.upserted', { deviceId: 'dev-test', channel: { id: 'edge4', name: 'edge4' } });
  presenceStore.hydrateLastActive('edge4', NOW - 4 * HOUR);
  bus.emit('channel.upserted', { deviceId: 'dev-test', channel: { id: 'edge72', name: 'edge72' } });
  presenceStore.hydrateLastActive('edge72', NOW - 72 * HOUR);
  const { primary, fallback } = buildAttentionList(NOW);
  assert.deepEqual(ids(primary), []);
  assert.deepEqual(ids(fallback), ['edge4']);
});

test('stable order: same inputs at two `now` values produce identical order until a boundary crosses', () => {
  seedChannel('a', 0.5);
  seedChannel('b', 1.0);
  seedChannel('c', 2.0);
  const a1 = ids(buildAttentionList(NOW).primary);
  const a2 = ids(buildAttentionList(NOW + 30 * 60 * 1000).primary);
  assert.deepEqual(a1, a2);
  assert.deepEqual(a1, ['c', 'b', 'a']);
});

test('live update: client-sent message bumps channel into primary group', () => {
  // Seed it deep in the fallback (40hr ago).
  seedChannel('ch', 40);
  // A user message arrives — presence-store's bus hook should advance
  // lastActiveAt to the message's created_at.
  bus.emit('message.received', {
    channelId: 'ch',
    msg: { sender: 'client', created_at: new Date(NOW - 5 * 60 * 1000).toISOString() },
  });
  const { primary, fallback } = buildAttentionList(NOW);
  assert.deepEqual(ids(primary), ['ch']);
  assert.deepEqual(ids(fallback), []);
});

test('channels with no activity stamp are excluded from both groups', () => {
  bus.emit('channel.upserted', { deviceId: 'dev-test', channel: { id: 'cold', name: 'cold' } });
  const { primary, fallback } = buildAttentionList(NOW);
  assert.deepEqual(ids(primary), []);
  assert.deepEqual(ids(fallback), []);
});

test('row metadata carries unread / interaction / running flags', () => {
  seedChannel('ch', 0.5);
  unreadStore.hydrate('ch', 3, true);
  bus.emit('agent.active', { channelId: 'ch', active: true });
  const { primary } = buildAttentionList(NOW);
  const row = primary.find(p => p.ch.id === 'ch');
  assert.ok(row);
  assert.equal(row.running, true);
  assert.equal(row.waitingInteraction, true);
  assert.equal(row.waitingUnread, true);
  assert.equal(row.count, 3);
});
