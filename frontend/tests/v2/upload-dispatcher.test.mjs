import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import { bindE2EEDispatcher } from '../../src/dashboard-v2/transport/e2ee-dispatcher.js';

function capture(types) {
  const seen = {};
  const offs = [];
  for (const t of types) {
    seen[t] = [];
    offs.push(bus.on(t, (p) => seen[t].push(p)));
  }
  return { seen, dispose: () => offs.forEach(fn => fn()) };
}

test('upload_progress maps to upload.progress with renamed fields', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'dev-u');
  const c = capture(['upload.progress']);

  fake.dispatchEvent(new CustomEvent('upload_progress', {
    detail: { file_id: 'f1', filename: 'doc.txt', progress: 0.5, total_chunks: 4, chunks_done: 2 },
  }));

  assert.equal(c.seen['upload.progress'].length, 1);
  assert.deepEqual(c.seen['upload.progress'][0], {
    deviceId: 'dev-u',
    fileId: 'f1',
    fileName: 'doc.txt',
    progress: 0.5,
    totalChunks: 4,
    chunksDone: 2,
  });
  c.dispose();
});

test('upload_done maps to upload.done with renamed fields', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'dev-u2');
  const c = capture(['upload.done']);

  fake.dispatchEvent(new CustomEvent('upload_done', {
    detail: { file_id: 'f1', filename: 'doc.txt', size: 1234 },
  }));

  assert.deepEqual(c.seen['upload.done'][0], {
    deviceId: 'dev-u2',
    fileId: 'f1',
    fileName: 'doc.txt',
    size: 1234,
  });
  c.dispose();
});
