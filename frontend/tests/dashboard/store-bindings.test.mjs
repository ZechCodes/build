// Smoke: each store's bus binding does the obvious thing.
// Keeps the "subscribe in the store file" wiring from silently breaking.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard/core/bus.js';
import {
  devicesStore, channelsStore, messagesStore, activityStore,
  presenceStore, unreadStore, filesStore, terminalStore,
  tasksStore, complicationsStore, uiStore,
} from '../../src/dashboard/domain/index.js';

test('device.bulk replaces devicesStore', () => {
  bus.emit('device.bulk', { devices: [{ id: 'd-sb1', name: 'one', status: 'online' }] });
  assert.equal(devicesStore.get('d-sb1').name, 'one');
});

test('device.status_changed updates status', () => {
  bus.emit('device.bulk', { devices: [{ id: 'd-sb2', name: 'two', status: 'online' }] });
  bus.emit('device.status_changed', { deviceId: 'd-sb2', status: 'offline' });
  assert.equal(devicesStore.get('d-sb2').status, 'offline');
});

test('channel.list + channel.upserted + channel.removed populate channelsStore', () => {
  bus.emit('channel.list', { deviceId: 'dev-sb', channels: [{ id: 'ch-sb1', name: 'a' }] });
  assert.equal(channelsStore.get('ch-sb1').name, 'a');
  assert.equal(channelsStore.deviceFor('ch-sb1'), 'dev-sb');

  bus.emit('channel.upserted', { deviceId: 'dev-sb', channel: { id: 'ch-sb2', name: 'b' } });
  assert.equal(channelsStore.get('ch-sb2').name, 'b');

  bus.emit('channel.removed', { deviceId: 'dev-sb', channelId: 'ch-sb1' });
  assert.equal(channelsStore.get('ch-sb1'), undefined);
});

test('channel.patched applies patch', () => {
  bus.emit('channel.upserted', { deviceId: 'dev-sb', channel: { id: 'ch-sb3', name: 'x' } });
  bus.emit('channel.patched', { channelId: 'ch-sb3', patch: { name: 'y' } });
  assert.equal(channelsStore.get('ch-sb3').name, 'y');
});

test('message.received appends into messagesStore', () => {
  bus.emit('message.received', {
    channelId: 'sb-m',
    msg: { id: 'sb-m1', content: 'hi', sender: 'client' },
  });
  assert.equal(messagesStore.forChannel('sb-m').length, 1);
  assert.equal(messagesStore.forChannel('sb-m')[0].content, 'hi');
});

test('message.bulk replaces messagesStore slice', () => {
  bus.emit('message.bulk', {
    channelId: 'sb-mb',
    msgs: [{ id: 'a' }, { id: 'b' }],
  });
  assert.equal(messagesStore.forChannel('sb-mb').length, 2);
});

test('agent.tool_use + agent.tool_result populate activityStore', () => {
  bus.emit('agent.tool_use', {
    channelId: 'sb-a', toolUseId: 't1', name: 'Read', input: { path: 'x' }, at: '2026-04-20',
  });
  bus.emit('agent.tool_result', {
    channelId: 'sb-a', toolUseId: 't1', isError: false, content: 'ok', at: '2026-04-20',
  });
  const list = activityStore.forChannel('sb-a');
  assert.equal(list.length, 1);
  assert.equal(list[0].name, 'Read');
  assert.equal(list[0].result.content, 'ok');
});

test('agent.reasoning appends + coalesces with previous reasoning entry', () => {
  bus.emit('agent.reasoning', { channelId: 'sb-r', text: 'thinking ', at: '2026-04-20' });
  bus.emit('agent.reasoning', { channelId: 'sb-r', text: 'more', at: '2026-04-20' });
  const list = activityStore.forChannel('sb-r');
  assert.equal(list.length, 1);
  assert.equal(list[0].text, 'thinking more');
});

test('agent.activity_history normalizes and replaces', () => {
  bus.emit('agent.activity_history', {
    channelId: 'sb-ah',
    entries: [
      { type: 'tool_use',    created_at: 't1', data: { id: 'x', name: 'Read', input: { p: 'a' } } },
      { type: 'tool_result', created_at: 't2', data: { tool_use_id: 'x', is_error: false, content: 'ok' } },
      { type: 'text',        created_at: 't3', data: { text: 'final thoughts' } },
    ],
  });
  const list = activityStore.forChannel('sb-ah');
  assert.equal(list.length, 2);
  assert.equal(list[0].name, 'Read');
  assert.equal(list[0].result.content, 'ok');
  assert.equal(list[1].text, 'final thoughts');
});

test('agent.started/stopped toggles presence', () => {
  bus.emit('agent.started', { channelId: 'sb-p' });
  assert.equal(presenceStore.get('sb-p').agentActive, true);
  bus.emit('agent.stopped', { channelId: 'sb-p' });
  assert.equal(presenceStore.get('sb-p').agentActive, false);
});

test('agent.plan_mode updates presence', () => {
  bus.emit('agent.plan_mode', { channelId: 'sb-pm', planMode: true });
  assert.equal(presenceStore.get('sb-pm').planMode, true);
});

test('harness.list stored by device', () => {
  bus.emit('harness.list', { deviceId: 'sb-dev-h', harnesses: [{ id: 'h1' }, { id: 'h2' }] });
  assert.equal(presenceStore.getHarnesses('sb-dev-h').length, 2);
});

test('unread increments on message for non-active channel', () => {
  uiStore.setActiveChannel(null);
  bus.emit('message.received', {
    channelId: 'sb-un',
    msg: { id: 'um1', sender: 'Device', content: 'hey' },
  });
  assert.equal(unreadStore.get('sb-un').count, 1);
});

test('unread does NOT increment for active channel', () => {
  uiStore.setActiveChannel('sb-un-active');
  unreadStore.markRead('sb-un-active');
  bus.emit('message.received', {
    channelId: 'sb-un-active',
    msg: { id: 'um2', sender: 'Device', content: 'hey' },
  });
  assert.equal(unreadStore.get('sb-un-active').count, 0);
  uiStore.setActiveChannel(null);
});

test('unread does NOT increment for client-sent message', () => {
  uiStore.setActiveChannel(null);
  const before = unreadStore.get('sb-un-self').count;
  bus.emit('message.received', {
    channelId: 'sb-un-self',
    msg: { id: 'um3', sender: 'client', content: 'mine' },
  });
  assert.equal(unreadStore.get('sb-un-self').count, before);
});

test('interaction.requested increments with hasInteraction', () => {
  uiStore.setActiveChannel(null);
  bus.emit('interaction.requested', { channelId: 'sb-ui' });
  assert.equal(unreadStore.get('sb-ui').hasInteraction, true);
});

test('files.list_result populates filesStore', () => {
  bus.emit('files.list_result', {
    channelId: 'sb-f', path: '', entries: [{ name: 'a.js' }], truncated: false,
  });
  assert.equal(filesStore.treeFor('sb-f').get('').entries.length, 1);
});

test('files.read_result populates readResultFor', () => {
  bus.emit('files.read_result', {
    channel_id: 'sb-fr', path: 'a.js', content: 'const x = 1;', size: 12, truncated: false,
  });
  const r = filesStore.readResultFor('sb-fr');
  assert.equal(r.path, 'a.js');
  assert.equal(r.content, 'const x = 1;');
});

test('files.diff_result populates diffResultFor', () => {
  bus.emit('files.diff_result', {
    channel_id: 'sb-fd', path: 'a.js', diff: '--- a\n+++ b\n', truncated: false,
  });
  const r = filesStore.diffResultFor('sb-fd');
  assert.match(r.diff, /^---/);
});

test('terminal.output + terminal.complete populate terminalStore', () => {
  bus.emit('terminal.output', { channelId: 'sb-t', text: 'out' });
  bus.emit('terminal.complete', { channelId: 'sb-t', exitCode: 0 });
  const slot = terminalStore.forChannel('sb-t');
  assert.equal(slot.history.length, 2);
  assert.equal(slot.running, false);
});

test('terminal.complete captures cwd when provided', () => {
  bus.emit('terminal.output', { channelId: 'sb-tc', text: 'x' });
  bus.emit('terminal.complete', { channelId: 'sb-tc', exitCode: 0, cwd: '/root/work' });
  const slot = terminalStore.forChannel('sb-tc');
  assert.equal(slot.cwd, '/root/work');
});

test('agent.todo_write sets tasks', () => {
  bus.emit('agent.todo_write', { channelId: 'sb-tw', todos: [{ text: 'one' }] });
  assert.equal(tasksStore.forChannel('sb-tw').length, 1);
});

test('complications.bulk + complication.upserted + complication.removed', () => {
  bus.emit('complications.bulk', { channelId: 'sb-c', complications: [{ id: 'c1' }] });
  bus.emit('complication.upserted', { channelId: 'sb-c', complication: { id: 'c2' } });
  bus.emit('complication.removed', { channelId: 'sb-c', complicationId: 'c1' });
  const list = complicationsStore.forChannel('sb-c');
  assert.equal(list.length, 1);
  assert.equal(list[0].id, 'c2');
});
