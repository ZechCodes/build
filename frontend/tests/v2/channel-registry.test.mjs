import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import { uiStore } from '../../src/dashboard-v2/domain/ui-store.js';
import { ChannelRegistry } from '../../src/dashboard-v2/channel/registry.js';

// Fake channel class — counts activate/deactivate/destroy calls. Uses a
// process-global monotonic counter for lastActivatedAt so LRU ordering
// is deterministic regardless of clock resolution.
let _tick = 0;

class FakeChannel {
  constructor(id) {
    this.id = id;
    this.active = false;
    this.activated = 0;
    this.deactivated = 0;
    this.destroyed = 0;
    this.viewState = { lastActivatedAt: 0 };
  }
  activate() {
    this.activated += 1;
    this.active = true;
    _tick += 1;
    this.viewState.lastActivatedAt = _tick;
  }
  deactivate() { this.deactivated += 1; this.active = false; }
  destroy() { this.destroyed += 1; this.deactivate(); }
}

function makeReg(opts = {}) {
  return new ChannelRegistry({ ChannelCls: FakeChannel, ...opts });
}

test('activate creates a channel and activates it', () => {
  const r = makeReg();
  r.activate('r-1');
  assert.equal(r.pool.size, 1);
  assert.equal(r.active.id, 'r-1');
  assert.equal(r.active.activated, 1);
});

test('activate switches active channel', () => {
  const r = makeReg();
  r.activate('r-a');
  r.activate('r-b');
  assert.equal(r.active.id, 'r-b');
  const a = r.pool.get('r-a');
  assert.equal(a.deactivated, 1);
  const b = r.pool.get('r-b');
  assert.equal(b.activated, 1);
});

test('activate is a no-op when already active', () => {
  const r = makeReg();
  r.activate('r-x');
  r.activate('r-x');
  assert.equal(r.pool.get('r-x').activated, 1);
});

test('activate(null) deactivates without removing', () => {
  const r = makeReg();
  r.activate('r-y');
  r.activate(null);
  assert.equal(r.active, null);
  assert.equal(r.pool.size, 1);
  assert.equal(r.pool.get('r-y').deactivated, 1);
});

test('init subscribes to uiStore and activates on change', () => {
  const r = makeReg();
  r.init();
  uiStore.setActiveChannel(null);
  uiStore.setActiveChannel('r-ui');
  assert.equal(r.active?.id, 'r-ui');
  uiStore.setActiveChannel(null);
  assert.equal(r.active, null);
  r.teardown();
});

test('channel.removed bus event evicts', () => {
  const r = makeReg();
  r.init();
  uiStore.setActiveChannel('r-ev');
  bus.emit('channel.removed', { channelId: 'r-ev' });
  assert.equal(r.pool.has('r-ev'), false);
  r.teardown();
});

test('LRU evicts oldest non-active when cap exceeded', () => {
  const r = makeReg({ maxPool: 3 });
  r.activate('r-L1');
  r.activate('r-L2');
  r.activate('r-L3');
  r.activate('r-L4');   // should evict r-L1
  assert.equal(r.pool.has('r-L1'), false);
  assert.equal(r.pool.has('r-L4'), true);
  assert.equal(r.active.id, 'r-L4');
});

test('active channel is never evicted by LRU', () => {
  const r = makeReg({ maxPool: 2 });
  r.activate('r-keep-active');
  r.activate('r-extra');
  // Pool now: [r-keep-active (inactive), r-extra (active)]. Activate one more.
  r.activate('r-third');
  // r-third is active, r-extra was more recent than r-keep-active, so r-keep-active evicts.
  assert.equal(r.pool.has('r-keep-active'), false);
  assert.equal(r.pool.has('r-third'), true);
});

test('deactivateAll clears active without destroying pool', () => {
  const r = makeReg();
  r.activate('r-d');
  r.deactivateAll();
  assert.equal(r.active, null);
  assert.equal(r.pool.size, 1);
});
