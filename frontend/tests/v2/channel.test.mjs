import { test } from 'node:test';
import assert from 'node:assert/strict';

// Channel calls its views' .activate/.deactivate. We inject stub views so
// this test runs without a DOM.

import { Channel } from '../../src/dashboard-v2/channel/channel.js';
import { bus } from '../../src/dashboard-v2/core/bus.js';
import { messagesStore } from '../../src/dashboard-v2/domain/messages-store.js';

function stubView() {
  return {
    activated: 0,
    deactivated: 0,
    activate() { this.activated += 1; },
    deactivate() { this.deactivated += 1; },
  };
}

test('activate mounts views and deactivate unmounts them', () => {
  const chat = stubView();
  const console_ = stubView();
  const ch = new Channel('chA', { views: { chat, console: console_ } });
  ch.activate();
  assert.equal(chat.activated, 1);
  assert.equal(console_.activated, 1);
  assert.equal(ch.active, true);

  ch.deactivate();
  assert.equal(chat.deactivated, 1);
  assert.equal(console_.deactivated, 1);
  assert.equal(ch.active, false);
});

test('activate is idempotent', () => {
  const v = stubView();
  const ch = new Channel('chB', { views: { v } });
  ch.activate();
  ch.activate();
  assert.equal(v.activated, 1);
});

test('destroy unmounts and nulls viewState/views', () => {
  const v = stubView();
  const ch = new Channel('chC', { views: { v } });
  ch.activate();
  ch.destroy();
  assert.equal(v.deactivated, 1);
  assert.equal(ch.viewState, null);
  assert.equal(ch.views, null);
});

test('activate requests history when messagesStore is empty', () => {
  const v = stubView();
  const ch = new Channel('chD', { views: { v } });
  const events = [];
  const off = bus.on('intent.get_messages', (p) => events.push(p));
  ch.activate();
  off();
  assert.ok(events.some(e => e.channelId === 'chD'));
});

test('activate does not request messages when history already cached', () => {
  const v = stubView();
  messagesStore.bulk('chE', [{ id: 'm1', content: 'cached' }]);
  const ch = new Channel('chE', { views: { v } });
  const events = [];
  const off = bus.on('intent.get_messages', (p) => events.push(p));
  ch.activate();
  off();
  assert.equal(events.filter(e => e.channelId === 'chE').length, 0);
});

test('viewState lastActivatedAt grows across activations', async () => {
  const v = stubView();
  const ch = new Channel('chF', { views: { v } });
  ch.activate();
  const first = ch.viewState.lastActivatedAt;
  ch.deactivate();
  await new Promise(r => setTimeout(r, 5));
  ch.activate();
  assert.ok(ch.viewState.lastActivatedAt >= first);
});
