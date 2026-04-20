import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import { filesStore } from '../../src/dashboard-v2/domain/files-store.js';

test('single-chunk image sets readResult directly', () => {
  bus.emit('files.read_result', {
    channel_id: 'img-1', path: 'a.png', is_image: true, content: 'data:image/png;base64,AAAA',
  });
  const r = filesStore.readResultFor('img-1');
  assert.equal(r.content, 'data:image/png;base64,AAAA');
  assert.ok(!r._progress);
});

test('chunked image accumulates until complete', () => {
  // 3 chunks
  bus.emit('files.read_result', {
    channel_id: 'img-2', path: 'big.png', is_image: true, content: 'aaa',
    chunk_index: 0, chunk_total: 3,
  });
  let r = filesStore.readResultFor('img-2');
  assert.ok(r._progress);
  assert.equal(r.chunk_received, 1);
  assert.equal(r.chunk_total, 3);

  bus.emit('files.read_result', {
    channel_id: 'img-2', path: 'big.png', is_image: true, content: 'bbb',
    chunk_index: 1, chunk_total: 3,
  });
  r = filesStore.readResultFor('img-2');
  assert.ok(r._progress);
  assert.equal(r.chunk_received, 2);

  bus.emit('files.read_result', {
    channel_id: 'img-2', path: 'big.png', is_image: true, content: 'ccc',
    chunk_index: 2, chunk_total: 3,
  });
  r = filesStore.readResultFor('img-2');
  assert.ok(!r._progress);
  assert.equal(r.content, 'aaabbbccc');
});

test('changing chunk_total resets the buffer', () => {
  bus.emit('files.read_result', {
    channel_id: 'img-3', path: 'x.png', is_image: true, content: 'a',
    chunk_index: 0, chunk_total: 2,
  });
  // Different total — should start over
  bus.emit('files.read_result', {
    channel_id: 'img-3', path: 'x.png', is_image: true, content: 'X',
    chunk_index: 0, chunk_total: 3,
  });
  const r = filesStore.readResultFor('img-3');
  assert.ok(r._progress);
  assert.equal(r.chunk_total, 3);
  assert.equal(r.chunk_received, 1);
});

test('non-image chunked result passes through unchanged', () => {
  bus.emit('files.read_result', {
    channel_id: 'text-1', path: 'a.txt', content: 'hello', size: 5,
  });
  const r = filesStore.readResultFor('text-1');
  assert.equal(r.content, 'hello');
  assert.ok(!r._progress);
});
