// chat-images-store.js — lazy chat-image cache.

import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard/core/bus.js';
import { chatImagesStore } from '../../src/dashboard/domain/chat-images-store.js';

beforeEach(() => { chatImagesStore.__resetForTests__(); });

test('get returns null for unknown (channel, path)', () => {
  assert.equal(chatImagesStore.get('chX', 'foo.png'), null);
});

test('request marks the record loading and emits intent.chat_image_fetch', () => {
  const fired = [];
  const off = bus.on('intent.chat_image_fetch', (p) => fired.push(p));
  chatImagesStore.request('ch1', 'a.png');
  off();
  assert.deepEqual(fired, [{ channelId: 'ch1', path: 'a.png' }]);
  assert.deepEqual(chatImagesStore.get('ch1', 'a.png'), { status: 'loading' });
});

test('repeated request on a loading entry does NOT re-fire the intent', () => {
  chatImagesStore.request('ch1', 'a.png');
  const fired = [];
  const off = bus.on('intent.chat_image_fetch', (p) => fired.push(p));
  chatImagesStore.request('ch1', 'a.png');
  off();
  assert.deepEqual(fired, []);
});

test('chat_image.received with dataUri flips state to ready', () => {
  chatImagesStore.request('ch1', 'a.png');
  bus.emit('chat_image.received', {
    channelId: 'ch1',
    path: 'a.png',
    dataUri: 'data:image/png;base64,XYZ',
  });
  assert.deepEqual(chatImagesStore.get('ch1', 'a.png'), {
    status: 'ready',
    dataUri: 'data:image/png;base64,XYZ',
  });
});

test('chat_image.received with error flips state to error', () => {
  chatImagesStore.request('ch1', 'a.png');
  bus.emit('chat_image.received', {
    channelId: 'ch1',
    path: 'a.png',
    error: 'Image too large',
  });
  assert.deepEqual(chatImagesStore.get('ch1', 'a.png'), {
    status: 'error',
    error: 'Image too large',
  });
});

test('multiple paths in the same channel coexist', () => {
  chatImagesStore.request('ch1', 'a.png');
  chatImagesStore.request('ch1', 'b.png');
  bus.emit('chat_image.received', { channelId: 'ch1', path: 'a.png', dataUri: 'data:image/png;base64,A' });
  assert.equal(chatImagesStore.get('ch1', 'a.png').status, 'ready');
  assert.equal(chatImagesStore.get('ch1', 'b.png').status, 'loading');
});

test('ready notify fires for subscribers', () => {
  const seen = [];
  const off = chatImagesStore.subscribe(e => seen.push(e));
  chatImagesStore.request('ch1', 'a.png');
  bus.emit('chat_image.received', { channelId: 'ch1', path: 'a.png', dataUri: 'data:image/png;base64,X' });
  off();
  const kinds = seen.map(e => e.kind);
  assert.ok(kinds.includes('requested'));
  assert.ok(kinds.includes('ready'));
});

test('after an error, a subsequent request retries (fires the intent again)', () => {
  chatImagesStore.request('ch1', 'a.png');
  bus.emit('chat_image.received', { channelId: 'ch1', path: 'a.png', error: 'oh no' });
  const fired = [];
  const off = bus.on('intent.chat_image_fetch', (p) => fired.push(p));
  chatImagesStore.request('ch1', 'a.png');
  off();
  assert.deepEqual(fired, [{ channelId: 'ch1', path: 'a.png' }]);
});
