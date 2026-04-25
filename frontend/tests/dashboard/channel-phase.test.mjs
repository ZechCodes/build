// ChannelPhaseView writes a `data-channel-phase` attribute onto the
// DOM so CSS can show loading skeletons and suppress empty states
// while the active channel is still mounting/loading.

import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

// jsdom-lite: a tiny DOM shim. Node's test runner doesn't bring a DOM.
// We only need querySelector + setAttribute/removeAttribute + a stub
// `document` that returns a fake .v2-app root.
const attrs = {};
const root = {
  setAttribute(name, value) { attrs[name] = value; },
  removeAttribute(name) { delete attrs[name]; },
  getAttribute(name) { return attrs[name]; },
};
globalThis.document = {
  querySelector(sel) { return sel === '.v2-app' ? root : null; },
  body: root,
};

import { bus } from '../../src/dashboard/core/bus.js';
import { uiStore } from '../../src/dashboard/domain/ui-store.js';
import { channelRegistry } from '../../src/dashboard/channel/registry.js';
import { ChannelPhaseView } from '../../src/dashboard/shell/channel-phase.js';

beforeEach(() => {
  for (const k of Object.keys(attrs)) delete attrs[k];
  uiStore.setActiveChannel(null);
  // Clear pool between tests.
  channelRegistry.pool.clear();
  channelRegistry.active = null;
});

test('idle when no active channel', () => {
  const v = new ChannelPhaseView();
  v.activate();
  assert.equal(attrs['data-channel-phase'], 'idle');
  v.deactivate();
});

test('loading when the active channel is not yet in ready', () => {
  const v = new ChannelPhaseView();
  v.activate();

  // Simulate a channel in the pool sitting at `loading`.
  channelRegistry.pool.set('ch-1', { phase: 'loading' });
  uiStore.setActiveChannel('ch-1');

  assert.equal(attrs['data-channel-phase'], 'loading');
  v.deactivate();
});

test('transition to ready flips the attribute', () => {
  const v = new ChannelPhaseView();
  v.activate();
  channelRegistry.pool.set('ch-r', { phase: 'loading' });
  uiStore.setActiveChannel('ch-r');
  assert.equal(attrs['data-channel-phase'], 'loading');
  bus.emit('channel.phase', { channelId: 'ch-r', phase: 'ready', prevPhase: 'loading' });
  assert.equal(attrs['data-channel-phase'], 'ready');
  v.deactivate();
});

test('phase events for non-active channels are ignored', () => {
  const v = new ChannelPhaseView();
  v.activate();
  channelRegistry.pool.set('ch-active', { phase: 'loading' });
  channelRegistry.pool.set('ch-other',  { phase: 'loading' });
  uiStore.setActiveChannel('ch-active');
  assert.equal(attrs['data-channel-phase'], 'loading');
  bus.emit('channel.phase', { channelId: 'ch-other', phase: 'ready' });
  assert.equal(attrs['data-channel-phase'], 'loading', 'non-active phase must not leak');
  v.deactivate();
});

test('switching active channel to one without a pool entry is `loading`', () => {
  const v = new ChannelPhaseView();
  v.activate();
  uiStore.setActiveChannel('ch-missing');
  assert.equal(attrs['data-channel-phase'], 'loading');
  v.deactivate();
});

test('clearing the active channel returns to idle', () => {
  const v = new ChannelPhaseView();
  v.activate();
  channelRegistry.pool.set('ch-x', { phase: 'ready' });
  uiStore.setActiveChannel('ch-x');
  assert.equal(attrs['data-channel-phase'], 'ready');
  uiStore.setActiveChannel(null);
  assert.equal(attrs['data-channel-phase'], 'idle');
  v.deactivate();
});

test('deactivate removes the attribute', () => {
  const v = new ChannelPhaseView();
  v.activate();
  uiStore.setActiveChannel(null);
  assert.equal(attrs['data-channel-phase'], 'idle');
  v.deactivate();
  assert.equal(attrs['data-channel-phase'], undefined);
});
