import { test } from 'node:test';
import assert from 'node:assert/strict';

// The router reads/writes `window.location.hash` and listens for
// `hashchange`. Provide a minimal stub before importing so module-level
// code binds against it.

let _listeners = [];
const fakeWindow = {
  location: { hash: '' },
  addEventListener(type, fn) { _listeners.push({ type, fn }); },
  removeEventListener(type, fn) { _listeners = _listeners.filter(e => e.fn !== fn); },
};
globalThis.window = fakeWindow;

const { Router } = await import('../../src/dashboard-v2/shell/router.js');
const { uiStore } = await import('../../src/dashboard-v2/domain/ui-store.js');

function resetUi() {
  uiStore.setTab('files');
  uiStore.setActiveChannel(null);
}

test('parse handles valid and invalid hashes', () => {
  const r = new Router();
  assert.deepEqual(r.parse(''), { tab: null, channelId: null });
  assert.deepEqual(r.parse('#chat/abc'), { tab: 'chat', channelId: 'abc' });
  assert.deepEqual(r.parse('#files'), { tab: 'files', channelId: null });
  assert.deepEqual(r.parse('#badtab/abc'), { tab: null, channelId: 'abc' });
});

test('navigate updates uiStore and hash', () => {
  resetUi();
  const r = new Router();
  r.navigate('chat', 'ch1');
  assert.equal(uiStore.getTab(), 'chat');
  assert.equal(uiStore.getActiveChannel(), 'ch1');
  assert.equal(fakeWindow.location.hash, 'chat/ch1');
});

test('navigate with invalid tab is rejected (uiStore unchanged)', () => {
  resetUi();
  const r = new Router();
  r.navigate('not-a-tab', 'ch1');
  assert.equal(uiStore.getTab(), 'files');
  assert.equal(uiStore.getActiveChannel(), null);
});

test('history stack: sequential navigations push, back/forward cycle', () => {
  resetUi();
  fakeWindow.location.hash = '';
  const r = new Router();
  r.navigate('chat', 'a');
  r.navigate('chat', 'b');
  r.navigate('files', 'b');

  r.back();
  assert.equal(uiStore.getActiveChannel(), 'b');
  assert.equal(uiStore.getTab(), 'chat');
  r.back();
  assert.equal(uiStore.getActiveChannel(), 'a');
  r.forward();
  assert.equal(uiStore.getActiveChannel(), 'b');
});

test('history does not duplicate when navigating to the same entry', () => {
  resetUi();
  const r = new Router();
  r.navigate('chat', 'x');
  r.navigate('chat', 'x');
  r.navigate('chat', 'x');
  assert.equal(r.history.length, 1);
});

test('history truncates forward entries when branching', () => {
  resetUi();
  const r = new Router();
  r.navigate('chat', 'a');
  r.navigate('chat', 'b');
  r.navigate('chat', 'c');
  r.back();                 // cursor at b
  r.navigate('chat', 'd');  // should truncate c, push d
  assert.deepEqual(r.history.map(h => h.channelId), ['a', 'b', 'd']);
});

test('init parses current hash and applies once', () => {
  resetUi();
  fakeWindow.location.hash = '#chat/loaded';
  const r = new Router();
  r.init();
  assert.equal(uiStore.getTab(), 'chat');
  assert.equal(uiStore.getActiveChannel(), 'loaded');
});
