// current-tool-store.js — unit coverage for the bus + presence wiring.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import { presenceStore } from '../../src/dashboard-v2/domain/presence-store.js';
import { currentToolStore, bindCurrentTool } from '../../src/dashboard-v2/domain/current-tool-store.js';

// bind() is idempotent — call it once up front; all tests share the
// module-level singletons.
bindCurrentTool();

test('agent.tool_use sets the current entry + notifies', () => {
  const ch = 'ct-use';
  const events = [];
  const off = currentToolStore.subscribe(e => events.push(e));
  bus.emit('agent.tool_use', { channelId: ch, toolUseId: 'a1', name: 'Read', input: { file_path: '/x' } });
  off();
  assert.deepEqual(currentToolStore.get(ch), { toolUseId: 'a1', name: 'Read', input: { file_path: '/x' }, at: undefined });
  assert.equal(events.length, 1);
  assert.equal(events[0].channelId, ch);
  assert.equal(events[0].entry.toolUseId, 'a1');
});

test('a newer tool_use replaces the previous entry', () => {
  const ch = 'ct-replace';
  bus.emit('agent.tool_use', { channelId: ch, toolUseId: 'r1', name: 'Read', input: {} });
  bus.emit('agent.tool_use', { channelId: ch, toolUseId: 'r2', name: 'Bash', input: { command: 'ls' } });
  assert.equal(currentToolStore.get(ch).toolUseId, 'r2');
  assert.equal(currentToolStore.get(ch).name, 'Bash');
});

test('matching tool_result clears the entry', () => {
  const ch = 'ct-result';
  bus.emit('agent.tool_use', { channelId: ch, toolUseId: 'm1', name: 'Read', input: {} });
  bus.emit('agent.tool_result', { channelId: ch, toolUseId: 'm1', isError: false, content: 'ok' });
  assert.equal(currentToolStore.get(ch), null);
});

test('tool_result with a stale toolUseId is ignored', () => {
  const ch = 'ct-stale';
  bus.emit('agent.tool_use', { channelId: ch, toolUseId: 's1', name: 'Read', input: {} });
  bus.emit('agent.tool_use', { channelId: ch, toolUseId: 's2', name: 'Bash', input: {} });
  // s1 finishes late → must NOT clear the live s2 entry.
  bus.emit('agent.tool_result', { channelId: ch, toolUseId: 's1', isError: false, content: '' });
  assert.equal(currentToolStore.get(ch).toolUseId, 's2');
});

test('agent going idle (agent_active=false) clears the entry', () => {
  const ch = 'ct-idle';
  bus.emit('agent.tool_use', { channelId: ch, toolUseId: 'i1', name: 'Read', input: {} });
  assert.equal(currentToolStore.get(ch).toolUseId, 'i1');
  presenceStore.setAgentActive(ch, true);   // ensure a real transition
  presenceStore.setAgentActive(ch, false);
  assert.equal(currentToolStore.get(ch), null);
});

test('agent going active does NOT clear', () => {
  const ch = 'ct-active';
  bus.emit('agent.tool_use', { channelId: ch, toolUseId: 'v1', name: 'Read', input: {} });
  // Force a false→true transition.
  presenceStore.setAgentActive(ch, false);
  presenceStore.setAgentActive(ch, true);
  assert.equal(currentToolStore.get(ch).toolUseId, 'v1');
});

test('per-channel isolation', () => {
  const a = 'ct-iso-a';
  const b = 'ct-iso-b';
  bus.emit('agent.tool_use', { channelId: a, toolUseId: 'ia', name: 'Read', input: {} });
  bus.emit('agent.tool_use', { channelId: b, toolUseId: 'ib', name: 'Bash', input: {} });
  assert.equal(currentToolStore.get(a).toolUseId, 'ia');
  assert.equal(currentToolStore.get(b).toolUseId, 'ib');
  bus.emit('agent.tool_result', { channelId: a, toolUseId: 'ia', isError: false, content: '' });
  assert.equal(currentToolStore.get(a), null);
  assert.equal(currentToolStore.get(b).toolUseId, 'ib');
});

test('no channelId is a no-op', () => {
  const before = currentToolStore.get('ct-none');
  bus.emit('agent.tool_use', { channelId: null, toolUseId: 'x', name: 'Read', input: {} });
  bus.emit('agent.tool_result', { channelId: null, toolUseId: 'x', isError: false, content: '' });
  assert.equal(currentToolStore.get('ct-none'), before);
});
