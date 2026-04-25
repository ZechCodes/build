import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard/core/bus.js';
import { uiStore } from '../../src/dashboard/domain/ui-store.js';
import { ChannelRegistry } from '../../src/dashboard/channel/registry.js';

// FakeChannel with mount/load/unload. Tracks a monotonic tick for LRU.
let _tick = 0;

class FakeChannel {
  constructor(id, opts = {}) {
    this.id = id;
    this.phase = 'unmounted';
    this.mounted = 0;
    this.unloaded = 0;
    this.destroyed = 0;
    this.viewState = { lastActivatedAt: 0 };
    this._unloadDelayMs = opts.unloadDelayMs || 0;
  }
  mount() {
    this.mounted += 1;
    this.phase = 'mounted';
    _tick += 1;
    this.viewState.lastActivatedAt = _tick;
  }
  async load() {
    this.phase = 'ready';
  }
  async unload() {
    if (this._unloadDelayMs) await new Promise(r => setTimeout(r, this._unloadDelayMs));
    this.unloaded += 1;
    this.phase = 'unmounted';
  }
  async destroy() { this.destroyed += 1; await this.unload(); }
  persistNow() {}
}

function makeReg(opts = {}) {
  return new ChannelRegistry({ ChannelCls: FakeChannel, ...opts });
}

test('activate creates a channel and mounts it', async () => {
  const r = makeReg();
  await r.activate('r-1');
  assert.equal(r.pool.size, 1);
  assert.equal(r.active.id, 'r-1');
  assert.equal(r.active.mounted, 1);
});

test('activate switches active channel, awaiting previous unload', async () => {
  const r = makeReg();
  await r.activate('r-a');
  const a = r.pool.get('r-a');
  await r.activate('r-b');
  assert.equal(r.active.id, 'r-b');
  assert.equal(a.unloaded, 1);
  assert.equal(r.pool.get('r-b').mounted, 1);
});

test('activate is a no-op when already active', async () => {
  const r = makeReg();
  await r.activate('r-x');
  await r.activate('r-x');
  assert.equal(r.pool.get('r-x').mounted, 1);
});

test('activate(null) unloads without removing from pool', async () => {
  const r = makeReg();
  await r.activate('r-y');
  await r.activate(null);
  assert.equal(r.active, null);
  assert.equal(r.pool.size, 1);
  assert.equal(r.pool.get('r-y').unloaded, 1);
});

test('last requested id wins when activations interleave during slow unload', async () => {
  const r = makeReg({ ChannelCls: class extends FakeChannel {
    constructor(id) { super(id, { unloadDelayMs: 20 }); }
  }});
  await r.activate('r-A');
  // Three activations race on r-A's unload (20ms). Final should win.
  const p1 = r.activate('r-B');
  const p2 = r.activate('r-C');
  const p3 = r.activate('r-B');
  await Promise.all([p1, p2, p3]);

  assert.equal(r.active.id, 'r-B');
  assert.equal(r.pool.get('r-A').unloaded, 1);
  // r-C should have either not mounted, or been unloaded immediately if it did.
  const c = r.pool.get('r-C');
  if (c) assert.equal(c.mounted, c.unloaded, 'r-C mounted/unloaded counts match');
});

test('init subscribes to uiStore and activates on change', async () => {
  const r = makeReg();
  r.init();
  uiStore.setActiveChannel(null);
  uiStore.setActiveChannel('r-ui');
  // uiStore fires sync; activate is async. Let the microtask settle.
  await new Promise(r2 => setTimeout(r2, 5));
  assert.equal(r.active?.id, 'r-ui');
  uiStore.setActiveChannel(null);
  await new Promise(r2 => setTimeout(r2, 5));
  assert.equal(r.active, null);
  await r.teardown();
});

test('channel.removed bus event evicts', async () => {
  const r = makeReg();
  r.init();
  uiStore.setActiveChannel('r-ev');
  await new Promise(r2 => setTimeout(r2, 5));
  bus.emit('channel.removed', { channelId: 'r-ev' });
  await new Promise(r2 => setTimeout(r2, 5));
  assert.equal(r.pool.has('r-ev'), false);
  await r.teardown();
});

test('LRU evicts oldest non-active when cap exceeded', async () => {
  const r = makeReg({ maxPool: 3 });
  await r.activate('r-L1');
  await r.activate('r-L2');
  await r.activate('r-L3');
  await r.activate('r-L4');
  await new Promise(r2 => setTimeout(r2, 5));  // let eviction destroy() settle
  assert.equal(r.pool.has('r-L1'), false);
  assert.equal(r.pool.has('r-L4'), true);
  assert.equal(r.active.id, 'r-L4');
});

test('active channel is never evicted by LRU', async () => {
  const r = makeReg({ maxPool: 2 });
  await r.activate('r-keep-active');
  await r.activate('r-extra');
  await r.activate('r-third');
  await new Promise(r2 => setTimeout(r2, 5));
  assert.equal(r.pool.has('r-keep-active'), false);
  assert.equal(r.pool.has('r-third'), true);
});

test('deactivateAll awaits unload and clears active', async () => {
  const r = makeReg();
  await r.activate('r-d');
  await r.deactivateAll();
  assert.equal(r.active, null);
  assert.equal(r.pool.size, 1);
  assert.equal(r.pool.get('r-d').unloaded, 1);
});
