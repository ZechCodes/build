// attention-hydrator.js: derive() + bus wiring.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import { unreadStore } from '../../src/dashboard-v2/domain/unread-store.js';
import {
  bindAttentionHydrator,
  derive,
} from '../../src/dashboard-v2/transport/attention-hydrator.js';

test('derive counts unread server messages, skipping client', () => {
  const out = derive([
    { sender: 'client', read_at: null },                       // skip
    { sender: 'Agent',  read_at: null },                       // +1
    { sender: 'Agent',  read_at: '2026-01-01T00:00:00Z' },     // read, skip
    { sender: 'Device', read_at: null },                       // +1
  ]);
  assert.deepEqual(out, { count: 2, hasInteraction: false });
});

test('derive detects pending interaction from object metadata', () => {
  const out = derive([
    {
      sender: 'Agent',
      read_at: null,
      metadata: { interaction_id: 'int-1', kind: 'plan_review' },
    },
  ]);
  assert.equal(out.hasInteraction, true);
});

test('derive detects pending interaction from JSON-string metadata', () => {
  const out = derive([
    {
      sender: 'Agent',
      read_at: null,
      metadata: JSON.stringify({ interaction_id: 'int-2', kind: 'question' }),
    },
  ]);
  assert.equal(out.hasInteraction, true);
});

test('derive ignores resolved interactions', () => {
  const out = derive([
    {
      sender: 'Agent',
      read_at: null,
      metadata: JSON.stringify({
        interaction_id: 'int-3',
        kind: 'plan_review',
        resolved_at: '2026-01-01T00:00:00Z',
      }),
    },
  ]);
  assert.equal(out.hasInteraction, false);
});

test('derive tolerates malformed metadata without throwing', () => {
  const out = derive([
    { sender: 'Agent', read_at: null, metadata: '{not json' },
  ]);
  assert.equal(out.count, 1);
  assert.equal(out.hasInteraction, false);
});

// ---- Wiring ----

bindAttentionHydrator();

test('channel.list emits intent.get_messages per channel', () => {
  const seen = [];
  const off = bus.on('intent.get_messages', (p) => seen.push(p));
  bus.emit('channel.list', {
    deviceId: 'dev-1',
    channels: [{ id: 'ch-a' }, { id: 'ch-b' }],
  });
  off();
  assert.equal(seen.length, 2);
  assert.deepEqual(
    seen.map(x => x.channelId).sort(),
    ['ch-a', 'ch-b'],
  );
  for (const c of seen) assert.equal(c.limit, 200);
});

test('message.bulk hydrates unreadStore with derived counts', () => {
  const ch = 'ah-bulk';
  // Prime a non-zero state first so we can confirm hydrate replaces it.
  unreadStore.hydrate(ch, 99, false);
  bus.emit('message.bulk', {
    channelId: ch,
    msgs: [
      { sender: 'Agent',  read_at: null },
      { sender: 'Agent',  read_at: null },
      {
        sender: 'Agent',
        read_at: null,
        metadata: { interaction_id: 'int-x' },
      },
      { sender: 'client', read_at: null },
    ],
  });
  const slot = unreadStore.get(ch);
  assert.equal(slot.count, 3);
  assert.equal(slot.hasInteraction, true);
});

test('message.bulk with no channelId is a no-op', () => {
  // Prime an existing channel; make sure a channelId-less bulk
  // doesn't overwrite anything.
  unreadStore.hydrate('ah-other', 7, false);
  bus.emit('message.bulk', { msgs: [{ sender: 'Agent', read_at: null }] });
  assert.equal(unreadStore.get('ah-other').count, 7);
});
