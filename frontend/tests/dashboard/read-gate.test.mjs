import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  canMarkMessageRead,
  deriveUnreadFromMessages,
  READ_INTERACTION_DELAY_MS,
} from '../../src/dashboard/domain/read-gate.js';

const msg = { id: 'm1', sender: 'Agent', read_at: null };

test('read gate rejects interactions before the post-appearance delay', () => {
  assert.equal(canMarkMessageRead({
    msg,
    appearedAt: 1000,
    lastInteractionAt: 1000 + READ_INTERACTION_DELAY_MS - 1,
    messageBottom: 200,
    viewportBottom: 200,
  }), false);
});

test('read gate rejects messages whose bottom is still below the viewport', () => {
  assert.equal(canMarkMessageRead({
    msg,
    appearedAt: 1000,
    lastInteractionAt: 1000 + READ_INTERACTION_DELAY_MS,
    messageBottom: 260,
    viewportBottom: 200,
  }), false);
});

test('read gate accepts unread non-client messages after delay and bottom reach', () => {
  assert.equal(canMarkMessageRead({
    msg,
    appearedAt: 1000,
    lastInteractionAt: 1000 + READ_INTERACTION_DELAY_MS,
    messageBottom: 200,
    viewportBottom: 200,
  }), true);
});

test('read gate ignores client and already-read messages', () => {
  assert.equal(canMarkMessageRead({
    msg: { id: 'client', sender: 'client' },
    appearedAt: 1000,
    lastInteractionAt: 2000,
    messageBottom: 100,
    viewportBottom: 100,
  }), false);
  assert.equal(canMarkMessageRead({
    msg: { id: 'read', sender: 'Agent', read_at: '2026-04-25T00:00:00Z' },
    appearedAt: 1000,
    lastInteractionAt: 2000,
    messageBottom: 100,
    viewportBottom: 100,
  }), false);
});

test('deriveUnreadFromMessages counts remaining unread and latest interaction state', () => {
  const result = deriveUnreadFromMessages([
    { id: 'old', sender: 'Agent', read_at: '2026-04-25T00:00:00Z' },
    { id: 'unread', sender: 'Agent', read_at: null },
    { id: 'self', sender: 'client', read_at: null },
    {
      id: 'interaction',
      sender: 'Agent',
      read_at: null,
      metadata: JSON.stringify({ interaction_id: 'interaction' }),
    },
  ]);
  assert.equal(result.count, 2);
  assert.equal(result.hasInteraction, true);
  assert.equal(result.latest.id, 'interaction');
});
