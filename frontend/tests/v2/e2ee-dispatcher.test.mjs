import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard-v2/core/bus.js';
import { bindE2EEDispatcher } from '../../src/dashboard-v2/transport/e2ee-dispatcher.js';

// Capture the N most recent emissions of each type.
function capture(types) {
  const seen = {};
  const offs = [];
  for (const t of types) {
    seen[t] = [];
    offs.push(bus.on(t, (p) => seen[t].push(p)));
  }
  return { seen, dispose: () => offs.forEach(off => off()) };
}

function fire(instance, type, detail) {
  instance.dispatchEvent(new CustomEvent(type, { detail }));
}

test('connected/disconnected map to e2ee.*', () => {
  const fake = new EventTarget();
  const deviceId = 'dev1';
  bindE2EEDispatcher(fake, deviceId);
  const c = capture(['e2ee.connected', 'e2ee.disconnected']);

  fake.dispatchEvent(new Event('connected'));
  fake.dispatchEvent(new Event('disconnected'));

  assert.deepEqual(c.seen['e2ee.connected'], [{ deviceId }]);
  assert.deepEqual(c.seen['e2ee.disconnected'], [{ deviceId }]);
  c.dispose();
});

test('channel_list fans out to channel.list', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'devA');
  const c = capture(['channel.list']);
  const channels = [{ id: 'ch1', name: 'one' }, { id: 'ch2', name: 'two' }];
  fire(fake, 'channel_list', { channels, agent_cwd: '/tmp' });
  assert.equal(c.seen['channel.list'].length, 1);
  assert.equal(c.seen['channel.list'][0].deviceId, 'devA');
  assert.deepEqual(c.seen['channel.list'][0].channels, channels);
  assert.equal(c.seen['channel.list'][0].agentCwd, '/tmp');
  c.dispose();
});

test('channel_created/updated/deleted translate correctly', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'devB');
  const c = capture(['channel.upserted', 'channel.patched', 'channel.removed']);

  fire(fake, 'channel_created', { id: 'ch99', name: 'new' });
  fire(fake, 'channel_renamed', { channel_id: 'ch99', name: 'renamed' });
  fire(fake, 'channel_updated', { channel_id: 'ch99', model: 'claude-4', effort: 'high' });
  fire(fake, 'channel_deleted', { channel_id: 'ch99' });

  assert.equal(c.seen['channel.upserted'][0].channel.name, 'new');
  assert.deepEqual(c.seen['channel.patched'][0].patch, { name: 'renamed' });
  assert.deepEqual(c.seen['channel.patched'][1].patch, { model: 'claude-4', effort: 'high' });
  assert.equal(c.seen['channel.removed'][0].channelId, 'ch99');
  c.dispose();
});

test('messages/message/delivered/read translate correctly', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'devC');
  const c = capture(['message.bulk', 'message.received', 'message.delivered', 'message.read', 'message.delivery_failed']);

  fire(fake, 'messages', { channel_id: 'cx', messages: [{ id: 'm1' }, { id: 'm2' }] });
  fire(fake, 'message', { id: 'm3', channel_id: 'cx', content: 'hi' });
  fire(fake, 'delivered', { channel_id: 'cx', message_id: 'm3' });
  fire(fake, 'read', { channel_id: 'cx', message_ids: ['m3'] });
  fire(fake, 'delivery_failed', { channel_id: 'cx', message_id: 'm4' });

  assert.equal(c.seen['message.bulk'][0].msgs.length, 2);
  assert.equal(c.seen['message.received'][0].msg.id, 'm3');
  assert.equal(c.seen['message.delivered'][0].msgId, 'm3');
  assert.deepEqual(c.seen['message.read'][0].msgIds, ['m3']);
  assert.equal(c.seen['message.delivery_failed'][0].msgId, 'm4');
  c.dispose();
});

test('agent_event chat.response fans to message.received + agent.active', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'devD');
  const c = capture(['message.received', 'agent.active']);

  fire(fake, 'agent_event', {
    channel_id: 'cz',
    event_type: 'chat.response',
    event: { id: 'agent-msg-1', sender: 'Agent', content: 'yo', created_at: '2026-04-20T00:00:00Z' },
  });

  assert.equal(c.seen['message.received'][0].channelId, 'cz');
  assert.equal(c.seen['message.received'][0].msg.content, 'yo');
  assert.deepEqual(c.seen['agent.active'][0], { channelId: 'cz', active: true });
  c.dispose();
});

test('agent_event tool.use with TodoWrite also emits agent.todo_write', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'devE');
  const c = capture(['agent.tool_use', 'agent.todo_write']);

  fire(fake, 'agent_event', {
    channel_id: 'ct',
    event_type: 'tool.use',
    event: { tool_use_id: 't1', name: 'TodoWrite', input: { todos: [{ text: 'do it' }] }, created_at: '2026-04-20T00:00:00Z' },
  });

  assert.equal(c.seen['agent.tool_use'][0].toolUseId, 't1');
  assert.equal(c.seen['agent.todo_write'][0].todos.length, 1);
  c.dispose();
});

test('agent_event interaction.request emits interaction + mirror message', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'devF');
  const c = capture(['interaction.requested', 'message.received']);

  fire(fake, 'agent_event', {
    channel_id: 'ci',
    event_type: 'interaction.request',
    event: {
      interaction_id: 'x1',
      kind: 'question',
      question: 'Pick one',
      options: ['a', 'b'],
      allow_freeform: true,
    },
  });

  assert.equal(c.seen['interaction.requested'][0].interactionId, 'x1');
  assert.equal(c.seen['interaction.requested'][0].options.length, 2);
  assert.equal(c.seen['message.received'][0].msg.id, 'x1');
  assert.ok(c.seen['message.received'][0].msg.metadata);
  c.dispose();
});

test('terminal_output streaming vs done translate correctly', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'devG');
  const c = capture(['terminal.output', 'terminal.complete']);

  fire(fake, 'terminal_output', { channel_id: 'cT', data: 'hello', done: false });
  fire(fake, 'terminal_output', { channel_id: 'cT', data: '', done: true, exit_code: 0, cwd: '/root' });

  assert.equal(c.seen['terminal.output'].length, 1);
  assert.equal(c.seen['terminal.output'][0].text, 'hello');
  assert.deepEqual(c.seen['terminal.complete'][0], { channelId: 'cT', exitCode: 0, cwd: '/root' });
  c.dispose();
});

test('files_list_result / files_changes_result translate', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'devH');
  const c = capture(['files.list_result', 'files.changes_result']);

  fire(fake, 'files_list_result', { channel_id: 'cf', path: 'src', entries: [{ name: 'a.js' }], truncated: false });
  fire(fake, 'files_changes_result', { channel_id: 'cf', repos: [{ path: '.', changes: [] }] });

  assert.equal(c.seen['files.list_result'][0].path, 'src');
  assert.equal(c.seen['files.list_result'][0].entries.length, 1);
  assert.equal(c.seen['files.changes_result'][0].repos.length, 1);
  c.dispose();
});

test('complications: update/remove/bulk', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'devI');
  const c = capture(['complication.upserted', 'complication.removed', 'complications.bulk']);

  fire(fake, 'complication_update', { channel_id: 'cc', id: 'k1', kind: 'git' });
  fire(fake, 'complication_remove', { channel_id: 'cc', id: 'k1' });
  fire(fake, 'complications', { channel_id: 'cc', complications: [{ id: 'k2' }] });

  assert.equal(c.seen['complication.upserted'][0].complication.id, 'k1');
  assert.equal(c.seen['complication.removed'][0].complicationId, 'k1');
  assert.equal(c.seen['complications.bulk'][0].complications.length, 1);
  c.dispose();
});

test('harness_list translates', () => {
  const fake = new EventTarget();
  bindE2EEDispatcher(fake, 'devJ');
  const c = capture(['harness.list']);
  fire(fake, 'harness_list', [{ id: 'h1', name: 'Claude' }]);
  assert.equal(c.seen['harness.list'][0].deviceId, 'devJ');
  assert.equal(c.seen['harness.list'][0].harnesses.length, 1);
  c.dispose();
});
