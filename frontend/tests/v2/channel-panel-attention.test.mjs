import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import { buildAttentionList, resetAttentionOrderForTests } from '../../src/dashboard-v2/shell/channel-panel.js';

function ids(items) {
  return items.map(item => item.ch.id);
}

test('attention list keeps stable insertion order and expires inactive channels', () => {
  const realNow = Date.now;
  let now = 1_800_000_000_000;
  Date.now = () => now;

  try {
    resetAttentionOrderForTests();
    bus.emit('channel.upserted', { deviceId: 'dev-attn-order', channel: { id: 'attn-a', name: 'A' } });
    bus.emit('channel.upserted', { deviceId: 'dev-attn-order', channel: { id: 'attn-b', name: 'B' } });
    bus.emit('agent.active', { channelId: 'attn-a', active: true });
    bus.emit('agent.active', { channelId: 'attn-b', active: true });

    assert.deepEqual(ids(buildAttentionList()).filter(id => id.startsWith('attn-')), ['attn-a', 'attn-b']);

    bus.emit('agent.active', { channelId: 'attn-a', active: false });
    bus.emit('agent.active', { channelId: 'attn-a', active: true });
    bus.emit('channel.upserted', { deviceId: 'dev-attn-order', channel: { id: 'attn-c', name: 'C' } });
    bus.emit('agent.active', { channelId: 'attn-c', active: true });

    assert.deepEqual(ids(buildAttentionList()).filter(id => id.startsWith('attn-')), ['attn-a', 'attn-b', 'attn-c']);

    bus.emit('agent.active', { channelId: 'attn-a', active: false });
    bus.emit('agent.active', { channelId: 'attn-b', active: false });
    bus.emit('agent.active', { channelId: 'attn-c', active: false });
    now += 61 * 60 * 1000;

    assert.deepEqual(ids(buildAttentionList()).filter(id => id.startsWith('attn-')), []);
  } finally {
    Date.now = realNow;
    resetAttentionOrderForTests();
  }
});
