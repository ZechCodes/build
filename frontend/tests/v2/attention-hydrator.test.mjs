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
  assert.equal(out.count, 2);
  assert.equal(out.hasInteraction, false);
});

test('derive counts unread against channel.last_seen_at even if read_at is stamped', () => {
  // User has visited up to T; then the agent sent a message at T+1h
  // which has a stale `read_at` stamp (e.g. the backend stamped it
  // during a prior mark_read call, but it postdates the user's
  // actual last-seen mark).
  const lastSeenAt = '2026-04-21T00:00:00Z';
  const newerAt   = '2026-04-21T01:00:00Z';
  const out = derive([
    { sender: 'Agent', read_at: lastSeenAt, created_at: newerAt },
  ], { last_seen_at: lastSeenAt });
  assert.equal(out.count, 1);
});

test('derive does NOT count messages older than last_seen_at', () => {
  const lastSeenAt = '2026-04-21T00:00:00Z';
  const olderAt   = '2026-04-20T23:00:00Z';
  const out = derive([
    { sender: 'Agent', read_at: null, created_at: olderAt },
  ], { last_seen_at: lastSeenAt });
  assert.equal(out.count, 0);
});

test('derive returns latestActivityMs from newest non-client message', () => {
  const out = derive([
    { sender: 'Agent',  read_at: null, created_at: '2026-04-20T00:00:00Z' },
    { sender: 'client', read_at: null, created_at: '2026-04-22T00:00:00Z' },
    { sender: 'Agent',  read_at: null, created_at: '2026-04-21T12:00:00Z' },
  ]);
  assert.equal(out.latestActivityMs, Date.parse('2026-04-21T12:00:00Z'));
});

test('derive without a channel falls back to !read_at', () => {
  // Ensure old callers (and the "channel not known yet" path) still
  // work.
  const out = derive([
    { sender: 'Agent', read_at: null },
    { sender: 'Agent', read_at: '2026-04-21T00:00:00Z' },
  ]);
  assert.equal(out.count, 1);
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

test('old unresolved interaction followed by a plain message does NOT flag', () => {
  const out = derive([
    { sender: 'Agent', read_at: null, metadata: { interaction_id: 'old-1' } },
    { sender: 'Agent', read_at: null, content: 'moved on with it' },
  ]);
  assert.equal(out.hasInteraction, false);
});

test('latest message is a resolved interaction → NOT flagged', () => {
  const out = derive([
    {
      sender: 'Agent',
      read_at: null,
      metadata: { interaction_id: 'r-1', resolved_at: '2026-01-01T00:00:00Z' },
    },
  ]);
  assert.equal(out.hasInteraction, false);
});

test('latest message is an unresolved interaction preceded by a resolved one → flagged', () => {
  const out = derive([
    {
      sender: 'Agent',
      read_at: null,
      metadata: { interaction_id: 'r-1', resolved_at: '2026-01-01T00:00:00Z' },
    },
    { sender: 'Agent', read_at: null, metadata: { interaction_id: 'u-1' } },
  ]);
  assert.equal(out.hasInteraction, true);
});

test('client message after an unresolved interaction does NOT supersede it', () => {
  const out = derive([
    { sender: 'Agent',  read_at: null, metadata: { interaction_id: 'pending' } },
    { sender: 'client', read_at: null, content: 'typing…' },
  ]);
  assert.equal(out.hasInteraction, true);
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
