// Unit tests for transport/channel-loader.js — the single place
// that knows "load a channel's initial data".

import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard/core/bus.js';
import { messagesStore } from '../../src/dashboard/domain/messages-store.js';
import { channelsStore } from '../../src/dashboard/domain/channels-store.js';
import { devicesStore } from '../../src/dashboard/domain/devices-store.js';
import { uiStore } from '../../src/dashboard/domain/ui-store.js';
import { initSessionStore, _resetForTests as resetSession }
  from '../../src/dashboard/core/session-store.js';
import { loadChannel } from '../../src/dashboard/transport/channel-loader.js';

function seedReady(channelId, deviceId) {
  channelsStore.upsert({ deviceId, channel: { id: channelId, name: channelId } });
  bus.emit('sse.connected', {});
  bus.emit('e2ee.connected', { deviceId });
}

function emitInitialResponses(channelId, { withMessages = true } = {}) {
  if (withMessages) bus.emit('message.bulk', { channelId, msgs: [] });
  bus.emit('files.list_result', { channelId, path: '', entries: [] });
}

beforeEach(() => {
  resetSession();
  initSessionStore({ devicesStore, channelsStore, uiStore });
});

test('fires all five initial-data intents', async () => {
  seedReady('chA', 'dev-chA');
  const intents = { msgs: 0, activity: 0, comp: 0, filesList: 0, filesChanges: 0 };
  const offs = [
    bus.on('intent.get_messages', (p) => { if (p.channelId === 'chA') intents.msgs++; }),
    bus.on('intent.get_activity', (p) => { if (p.channelId === 'chA') intents.activity++; }),
    bus.on('intent.get_complications', (p) => { if (p.channelId === 'chA') intents.comp++; }),
    bus.on('intent.files_list', (p) => { if (p.channelId === 'chA') intents.filesList++; }),
    bus.on('intent.files_changes', (p) => { if (p.channelId === 'chA') intents.filesChanges++; }),
  ];

  const loadP = loadChannel('chA', { timeoutMs: 100 });
  await new Promise(r => setTimeout(r, 5));
  emitInitialResponses('chA');
  await loadP;
  offs.forEach(fn => fn());

  assert.equal(intents.msgs, 1);
  assert.equal(intents.activity, 1);
  assert.equal(intents.comp, 1);
  assert.equal(intents.filesList, 1);
  assert.equal(intents.filesChanges, 1);
});

test('skips get_messages when messagesStore has data', async () => {
  seedReady('chCached', 'dev-chCached');
  messagesStore.bulk('chCached', [{ id: 'm1', content: 'cached' }]);
  let msgs = 0;
  const off = bus.on('intent.get_messages', (p) => { if (p.channelId === 'chCached') msgs++; });

  const loadP = loadChannel('chCached', { timeoutMs: 100 });
  await new Promise(r => setTimeout(r, 5));
  emitInitialResponses('chCached', { withMessages: false });
  await loadP;
  off();

  assert.equal(msgs, 0);
});

test('forceFetchMessages re-fires get_messages even when cached', async () => {
  seedReady('chForce', 'dev-chForce');
  messagesStore.bulk('chForce', [{ id: 'm1', content: 'cached' }]);
  let msgs = 0;
  const off = bus.on('intent.get_messages', (p) => { if (p.channelId === 'chForce') msgs++; });

  const loadP = loadChannel('chForce', { timeoutMs: 100, forceFetchMessages: true });
  await new Promise(r => setTimeout(r, 5));
  emitInitialResponses('chForce');  // need message.bulk since we forced the fetch
  await loadP;
  off();

  assert.equal(msgs, 1);
});

test('resolves on timeout when responses never arrive', async () => {
  seedReady('chTimeout', 'dev-chTimeout');
  const t0 = Date.now();
  await loadChannel('chTimeout', { timeoutMs: 60 });
  const dt = Date.now() - t0;
  assert.ok(dt >= 50, `expected to hit timeout (~60ms), took ${dt}ms`);
});

test('rejects with AbortError when signal aborts', async () => {
  bus.emit('sse.connected', {});
  bus.emit('e2ee.connected', { deviceId: 'dev-abort' });
  const ac = new AbortController();
  const p = loadChannel('chAbort', { signal: ac.signal, timeoutMs: 1000 });
  ac.abort();
  // awaitChannelReady is the first await and will reject with AbortError.
  await assert.rejects(p, (err) => err.name === 'AbortError');
});

test('waits for channel→device resolution before firing intents', async () => {
  // Session is up for dev-late, but channelsStore doesn't map chLate yet.
  bus.emit('sse.connected', {});
  bus.emit('e2ee.connected', { deviceId: 'dev-late' });
  let filesList = 0;
  const off = bus.on('intent.files_list', (p) => { if (p.channelId === 'chLate') filesList++; });

  const loadP = loadChannel('chLate', { timeoutMs: 200 });
  await new Promise(r => setTimeout(r, 10));
  assert.equal(filesList, 0, 'intents must wait for channel→device resolution');

  channelsStore.upsert({ deviceId: 'dev-late', channel: { id: 'chLate', name: 'late' } });
  await new Promise(r => setTimeout(r, 5));
  emitInitialResponses('chLate');
  await loadP;
  off();

  assert.equal(filesList, 1);
});
