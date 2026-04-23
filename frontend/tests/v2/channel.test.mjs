import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

import { Channel } from '../../src/dashboard-v2/channel/channel.js';
import { bus } from '../../src/dashboard-v2/core/bus.js';
import { messagesStore } from '../../src/dashboard-v2/domain/messages-store.js';
import { channelsStore } from '../../src/dashboard-v2/domain/channels-store.js';
import { devicesStore } from '../../src/dashboard-v2/domain/devices-store.js';
import { uiStore } from '../../src/dashboard-v2/domain/ui-store.js';
import { initSessionStore, _resetForTests as resetSession }
  from '../../src/dashboard-v2/core/session-store.js';

function stubView() {
  return {
    activated: 0,
    deactivated: 0,
    activate() { this.activated += 1; },
    deactivate() { this.deactivated += 1; },
  };
}

function seedReady(channelId, deviceId) {
  channelsStore.upsert({ deviceId, channel: { id: channelId, name: channelId } });
  bus.emit('sse.connected', {});
  bus.emit('e2ee.connected', { deviceId });
}

beforeEach(() => {
  resetSession();
  initSessionStore({ devicesStore, channelsStore, uiStore });
});

test('mount activates views without fetching', () => {
  const chat = stubView();
  const ch = new Channel('chA', { views: { chat } });
  const intents = [];
  const off = bus.on('intent.get_messages', (p) => intents.push(p));
  ch.mount();
  off();
  assert.equal(chat.activated, 1);
  assert.equal(ch.phase, 'mounted');
  assert.equal(intents.length, 0);
});

test('load fires initial-data intents once session is ready', async () => {
  const chat = stubView();
  const ch = new Channel('chL', { views: { chat } });
  seedReady('chL', 'dev-chL');

  const intents = {
    messages: 0, activity: 0, complications: 0, filesList: 0, filesChanges: 0,
  };
  const offs = [
    bus.on('intent.get_messages', (p) => { if (p.channelId === 'chL') intents.messages++; }),
    bus.on('intent.get_activity', (p) => { if (p.channelId === 'chL') intents.activity++; }),
    bus.on('intent.get_complications', (p) => { if (p.channelId === 'chL') intents.complications++; }),
    bus.on('intent.files_list', (p) => { if (p.channelId === 'chL') intents.filesList++; }),
    bus.on('intent.files_changes', (p) => { if (p.channelId === 'chL') intents.filesChanges++; }),
  ];

  ch.mount();
  await ch.load();
  offs.forEach(fn => fn());

  assert.equal(ch.phase, 'ready');
  assert.equal(intents.messages, 1);
  assert.equal(intents.activity, 1);
  assert.equal(intents.complications, 1);
  assert.equal(intents.filesList, 1);
  assert.equal(intents.filesChanges, 1);
});

test('load skips get_messages when history is already cached', async () => {
  const chat = stubView();
  seedReady('chCached', 'dev-chCached');
  messagesStore.bulk('chCached', [{ id: 'm1', content: 'cached' }]);

  const ch = new Channel('chCached', { views: { chat } });
  let messagesCalls = 0;
  const off = bus.on('intent.get_messages', (p) => { if (p.channelId === 'chCached') messagesCalls++; });
  ch.mount();
  await ch.load();
  off();

  assert.equal(messagesCalls, 0);
});

test('load waits for the channel to resolve to a device', async () => {
  const chat = stubView();
  bus.emit('sse.connected', {});
  bus.emit('e2ee.connected', { deviceId: 'dev-late' });
  // Note: channelsStore DOES NOT yet map chLate → dev-late.

  const ch = new Channel('chLate', { views: { chat } });
  let filesCalls = 0;
  const off = bus.on('intent.files_list', (p) => { if (p.channelId === 'chLate') filesCalls++; });

  ch.mount();
  const loadPromise = ch.load();
  // Give the pending awaitChannelReady a tick. It should still be waiting.
  await new Promise(r => setTimeout(r, 5));
  assert.equal(filesCalls, 0, 'intents must wait for channel→device resolution');
  assert.equal(ch.phase, 'loading');

  channelsStore.upsert({ deviceId: 'dev-late', channel: { id: 'chLate', name: 'late' } });
  await loadPromise;
  off();

  assert.equal(ch.phase, 'ready');
  assert.equal(filesCalls, 1);
});

test('unload aborts an in-flight load', async () => {
  const chat = stubView();
  // Session is "up" for the device but channelsStore hasn't resolved yet,
  // so load() will park inside awaitChannelReady.
  bus.emit('sse.connected', {});
  bus.emit('e2ee.connected', { deviceId: 'dev-abort' });

  const ch = new Channel('chAbort', { views: { chat } });
  let filesCalls = 0;
  const off = bus.on('intent.files_list', (p) => { if (p.channelId === 'chAbort') filesCalls++; });

  ch.mount();
  const loadP = ch.load();
  await new Promise(r => setTimeout(r, 5));
  assert.equal(ch.phase, 'loading');
  await ch.unload();
  off();

  // load() should reject with AbortError; swallow it here.
  await loadP.catch(err => {
    assert.equal(err.name, 'AbortError');
  });

  assert.equal(ch.phase, 'unmounted');
  assert.equal(filesCalls, 0, 'no intents after abort');
  assert.equal(chat.deactivated, 1);
});

test('mount is idempotent', () => {
  const v = stubView();
  const ch = new Channel('chB', { views: { v } });
  ch.mount();
  ch.mount();
  assert.equal(v.activated, 1);
});

test('load is idempotent while in-flight', async () => {
  const chat = stubView();
  seedReady('chIdem', 'dev-chIdem');
  const ch = new Channel('chIdem', { views: { chat } });
  ch.mount();
  const p1 = ch.load();
  const p2 = ch.load();
  assert.equal(p1, p2, 'load() returns the same promise when called twice');
  await p1;
});

test('destroy unloads and nulls viewState/views', async () => {
  const v = stubView();
  const ch = new Channel('chC', { views: { v } });
  ch.mount();
  await ch.destroy();
  assert.equal(v.deactivated, 1);
  assert.equal(ch.viewState, null);
  assert.equal(ch.views, null);
});

test('viewState lastActivatedAt grows across mounts', async () => {
  const v = stubView();
  const ch = new Channel('chF', { views: { v } });
  ch.mount();
  const first = ch.viewState.lastActivatedAt;
  await ch.unload();
  await new Promise(r => setTimeout(r, 5));
  ch.mount();
  assert.ok(ch.viewState.lastActivatedAt >= first);
});

test('phase subscribe fires once on subscribe and on every transition', async () => {
  const v = stubView();
  seedReady('chPhase', 'dev-chPhase');
  const ch = new Channel('chPhase', { views: { v } });
  const phases = [];
  ch.subscribe(e => phases.push(e.phase));
  assert.deepEqual(phases, ['unmounted']);
  ch.mount();
  assert.deepEqual(phases, ['unmounted', 'mounting', 'mounted']);
  await ch.load();
  assert.deepEqual(phases, ['unmounted', 'mounting', 'mounted', 'loading', 'ready']);
  await ch.unload();
  assert.deepEqual(phases,
    ['unmounted', 'mounting', 'mounted', 'loading', 'ready', 'unloading', 'unmounted']);
});
