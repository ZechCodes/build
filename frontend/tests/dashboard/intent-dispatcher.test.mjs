// Verify intent.* events reach the right BuildE2EE method on the pool.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bus } from '../../src/dashboard/core/bus.js';
import { e2eePool } from '../../src/dashboard/transport/e2ee-pool.js';
import { bindIntentDispatcher } from '../../src/dashboard/transport/intent-dispatcher.js';

// Stub conn: records every method call as {method, args}.
function makeStub() {
  const calls = [];
  const rec = (method) => (...args) => { calls.push({ method, args }); };
  return {
    calls,
    connected: true,
    send:           rec('send'),
    sendMessage:    rec('sendMessage'),
    stopAgent:      rec('stopAgent'),
    restartAgent:   rec('restartAgent'),
    updateChannel:  rec('updateChannel'),
    deleteChannel:  rec('deleteChannel'),
    renameChannel:  rec('renameChannel'),
    markRead:       rec('markRead'),
    markSeen:       rec('markSeen'),
    resetSession:   rec('resetSession'),
    compactSession: rec('compactSession'),
    sendInteractionResponse: rec('sendInteractionResponse'),
    terminalExec:     rec('terminalExec'),
    terminalKill:     rec('terminalKill'),
    terminalComplete: rec('terminalComplete'),
    filesList:        rec('filesList'),
    filesChanges:     rec('filesChanges'),
    fileRead:         rec('fileRead'),
    fileDiff:         rec('fileDiff'),
    getMessages:      rec('getMessages'),
    getActivity:      rec('getActivity'),
    getComplications: rec('getComplications'),
  };
}

// Monkey-patch forChannel to return our stub. Bind once.
const stub = makeStub();
const origForChannel = e2eePool.forChannel;
e2eePool.forChannel = () => stub;
bindIntentDispatcher();

// Restore pool.forChannel at process exit so no other test is affected.
// (node:test runs tests serially in a single process; leaving the patch
// in place is fine for this isolated suite.)

async function emitAndAssert(type, payload, expectedMethod, expectedArgs) {
  const before = stub.calls.length;
  bus.emit(type, payload);
  // intent handlers are async — yield once so they dispatch to the stub.
  await new Promise((resolve) => setImmediate(resolve));
  const call = stub.calls[before];
  assert.ok(call, `no call captured for ${type}`);
  assert.equal(call.method, expectedMethod, `expected ${expectedMethod}, got ${call.method}`);
  if (expectedArgs !== undefined) {
    assert.deepEqual(call.args, expectedArgs);
  }
}

test('intent.send_message → send(payload)', async () => {
  await emitAndAssert(
    'intent.send_message',
    { channelId: 'ch1', text: 'hi' },
    'send',
    [{ action: 'message', channel_id: 'ch1', content: 'hi' }],
  );
});

test('intent.send_message with attachments + plan mode', async () => {
  await emitAndAssert(
    'intent.send_message',
    {
      channelId: 'ch1',
      text: 'hi',
      attachments: [{ file_id: 'f1', filename: 'a.txt', size: 5, mime_type: 'text/plain' }],
      planMode: true,
    },
    'send',
    [{
      action: 'message',
      channel_id: 'ch1',
      content: 'hi',
      attachments: [{ file_id: 'f1', filename: 'a.txt', size: 5, mime_type: 'text/plain' }],
      plan_mode: true,
    }],
  );
});

test('intent.stop_agent → stopAgent(channelId)', async () => {
  await emitAndAssert('intent.stop_agent', { channelId: 'ch1' }, 'stopAgent', ['ch1']);
});

test('intent.restart_agent → restartAgent(channelId)', async () => {
  await emitAndAssert('intent.restart_agent', { channelId: 'ch1' }, 'restartAgent', ['ch1']);
});

test('intent.update_channel → updateChannel(channelId, patch)', async () => {
  await emitAndAssert('intent.update_channel', { channelId: 'ch1', patch: { model: 'm1' } }, 'updateChannel', ['ch1', { model: 'm1' }]);
});

test('intent.delete_channel → deleteChannel(channelId)', async () => {
  await emitAndAssert('intent.delete_channel', { channelId: 'ch1' }, 'deleteChannel', ['ch1']);
});

test('intent.rename_channel → renameChannel(channelId, name)', async () => {
  await emitAndAssert('intent.rename_channel', { channelId: 'ch1', name: 'new' }, 'renameChannel', ['ch1', 'new']);
});

test('intent.mark_read → markRead(msgIds)', async () => {
  await emitAndAssert('intent.mark_read', { channelId: 'ch1', msgIds: ['m1', 'm2'] }, 'markRead', [['m1', 'm2']]);
});

test('intent.mark_seen → markSeen(channelId)', async () => {
  await emitAndAssert('intent.mark_seen', { channelId: 'ch1' }, 'markSeen', ['ch1']);
});

test('intent.reset_session → resetSession(channelId)', async () => {
  await emitAndAssert('intent.reset_session', { channelId: 'ch1' }, 'resetSession', ['ch1']);
});

test('intent.compact_session → compactSession(channelId)', async () => {
  await emitAndAssert('intent.compact_session', { channelId: 'ch1' }, 'compactSession', ['ch1']);
});

test('intent.interaction_response → sendInteractionResponse', async () => {
  await emitAndAssert(
    'intent.interaction_response',
    { channelId: 'ch1', interactionId: 'x', selectedOption: 'a', freeformResponse: null, selectedOptions: null },
    'sendInteractionResponse',
    ['ch1', 'x', 'a', null, null, undefined],
  );
});

test('intent.interaction_response forwards stepAnswers for paginated questions', async () => {
  const steps = [{ header: 'Q1', answer: 'yes' }, { header: 'Q2', answer: 'no' }];
  await emitAndAssert(
    'intent.interaction_response',
    { channelId: 'ch1', interactionId: 'x', selectedOption: null, freeformResponse: null, selectedOptions: null, stepAnswers: steps },
    'sendInteractionResponse',
    ['ch1', 'x', null, null, null, steps],
  );
});

test('intent.terminal_exec → terminalExec(channelId, command, cwd, commandId)', async () => {
  await emitAndAssert(
    'intent.terminal_exec',
    { channelId: 'ch1', command: 'ls', cwd: '/tmp', commandId: 'c1' },
    'terminalExec',
    ['ch1', 'ls', '/tmp', 'c1'],
  );
});

test('intent.files_list → filesList(channelId, path)', async () => {
  await emitAndAssert('intent.files_list', { channelId: 'ch1', path: 'src' }, 'filesList', ['ch1', 'src']);
});

test('intent.file_read → fileRead(channelId, path, offset, limit)', async () => {
  await emitAndAssert('intent.file_read', { channelId: 'ch1', path: 'a.js', offset: 0, limit: 100 }, 'fileRead', ['ch1', 'a.js', 0, 100]);
});

test('intent.get_messages → getMessages(channelId, limit, before)', async () => {
  await emitAndAssert('intent.get_messages', { channelId: 'ch1', limit: 50, before: null }, 'getMessages', ['ch1', 50, null]);
});

// Cleanup — restore the real forChannel so subsequent suites (even though
// node:test does not guarantee order) aren't affected.
test.after(() => {
  e2eePool.forChannel = origForChannel;
});
