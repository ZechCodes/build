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

/** Simulate the bridge replying to the initial-data intents so
 *  Channel.load() can transition to `ready`. */
function emitInitialResponses(channelId, { withMessages = true } = {}) {
  if (withMessages) {
    bus.emit('message.bulk', { channelId, msgs: [] });
  }
  bus.emit('files.list_result', { channelId, path: '', entries: [] });
}

function makeChannel(id, opts) {
  const ch = new Channel(id, opts);
  // Short timeout so tests don't wait 5s for the stranded-response safety net.
  ch._loadTimeoutMs = 100;
  return ch;
}

beforeEach(() => {
  resetSession();
  initSessionStore({ devicesStore, channelsStore, uiStore });
});

test('mount activates views without fetching', () => {
  const chat = stubView();
  const ch = makeChannel('chA', { views: { chat } });
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
  const ch = makeChannel('chL', { views: { chat } });
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
  const loadP = ch.load();
  // After session-ready, intents fire; give the microtask a moment.
  await new Promise(r => setTimeout(r, 5));
  emitInitialResponses('chL');
  await loadP;
  offs.forEach(fn => fn());

  assert.equal(ch.phase, 'ready');
  assert.equal(intents.messages, 1);
  assert.equal(intents.activity, 1);
  assert.equal(intents.complications, 1);
  assert.equal(intents.filesList, 1);
  assert.equal(intents.filesChanges, 1);
});

test('load reaches ready on timeout even if responses never arrive', async () => {
  const chat = stubView();
  const ch = makeChannel('chTimeout', { views: { chat } });
  seedReady('chTimeout', 'dev-chTimeout');

  ch.mount();
  const t0 = Date.now();
  await ch.load();
  const dt = Date.now() - t0;

  assert.equal(ch.phase, 'ready');
  assert.ok(dt >= 90, `expected to hit timeout (~100ms), took ${dt}ms`);
});

test('load skips get_messages when history is already cached', async () => {
  const chat = stubView();
  seedReady('chCached', 'dev-chCached');
  messagesStore.bulk('chCached', [{ id: 'm1', content: 'cached' }]);

  const ch = makeChannel('chCached', { views: { chat } });
  let messagesCalls = 0;
  const off = bus.on('intent.get_messages', (p) => { if (p.channelId === 'chCached') messagesCalls++; });
  ch.mount();
  const loadP = ch.load();
  await new Promise(r => setTimeout(r, 5));
  // Only files.list_result needed now since get_messages wasn't fired.
  emitInitialResponses('chCached', { withMessages: false });
  await loadP;
  off();

  assert.equal(messagesCalls, 0);
});

test('load waits for the channel to resolve to a device', async () => {
  const chat = stubView();
  bus.emit('sse.connected', {});
  bus.emit('e2ee.connected', { deviceId: 'dev-late' });

  const ch = makeChannel('chLate', { views: { chat } });
  let filesCalls = 0;
  const off = bus.on('intent.files_list', (p) => { if (p.channelId === 'chLate') filesCalls++; });

  ch.mount();
  const loadPromise = ch.load();
  await new Promise(r => setTimeout(r, 5));
  assert.equal(filesCalls, 0, 'intents must wait for channel→device resolution');
  assert.equal(ch.phase, 'loading');

  channelsStore.upsert({ deviceId: 'dev-late', channel: { id: 'chLate', name: 'late' } });
  await new Promise(r => setTimeout(r, 5));
  emitInitialResponses('chLate');
  await loadPromise;
  off();

  assert.equal(ch.phase, 'ready');
  assert.equal(filesCalls, 1);
});

test('unload aborts an in-flight load', async () => {
  const chat = stubView();
  bus.emit('sse.connected', {});
  bus.emit('e2ee.connected', { deviceId: 'dev-abort' });

  const ch = makeChannel('chAbort', { views: { chat } });
  let filesCalls = 0;
  const off = bus.on('intent.files_list', (p) => { if (p.channelId === 'chAbort') filesCalls++; });

  ch.mount();
  const loadP = ch.load();
  await new Promise(r => setTimeout(r, 5));
  assert.equal(ch.phase, 'loading');
  await ch.unload();
  off();

  await loadP.catch(err => {
    assert.equal(err.name, 'AbortError');
  });

  assert.equal(ch.phase, 'unmounted');
  assert.equal(filesCalls, 0, 'no intents after abort');
  assert.equal(chat.deactivated, 1);
});

test('mount is idempotent', () => {
  const v = stubView();
  const ch = makeChannel('chB', { views: { v } });
  ch.mount();
  ch.mount();
  assert.equal(v.activated, 1);
});

test('load is idempotent while in-flight', async () => {
  const chat = stubView();
  seedReady('chIdem', 'dev-chIdem');
  const ch = makeChannel('chIdem', { views: { chat } });
  ch.mount();
  const p1 = ch.load();
  const p2 = ch.load();
  assert.equal(p1, p2, 'load() returns the same promise when called twice');
  await new Promise(r => setTimeout(r, 5));
  emitInitialResponses('chIdem');
  await p1;
});

test('destroy unloads and nulls viewState/views', async () => {
  const v = stubView();
  const ch = makeChannel('chC', { views: { v } });
  ch.mount();
  await ch.destroy();
  assert.equal(v.deactivated, 1);
  assert.equal(ch.viewState, null);
  assert.equal(ch.views, null);
});

test('viewState lastActivatedAt grows across mounts', async () => {
  const v = stubView();
  const ch = makeChannel('chF', { views: { v } });
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
  const ch = makeChannel('chPhase', { views: { v } });
  const phases = [];
  ch.subscribe(e => phases.push(e.phase));
  assert.deepEqual(phases, ['unmounted']);
  ch.mount();
  assert.deepEqual(phases, ['unmounted', 'mounting', 'mounted']);
  const loadP = ch.load();
  await new Promise(r => setTimeout(r, 5));
  emitInitialResponses('chPhase');
  await loadP;
  assert.deepEqual(phases, ['unmounted', 'mounting', 'mounted', 'loading', 'ready']);
  await ch.unload();
  assert.deepEqual(phases,
    ['unmounted', 'mounting', 'mounted', 'loading', 'ready', 'unloading', 'unmounted']);
});
