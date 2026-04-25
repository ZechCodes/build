// presence-store.js — hydrateLastActive() unit coverage.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { presenceStore } from '../../src/dashboard/domain/presence-store.js';

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
