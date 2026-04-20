// Consumes `intent.*` bus events from views and invokes the right
// BuildE2EE method on the appropriate pool instance.
//
// Views never import `e2eePool` or `BuildE2EE`. They emit intents.

import { bus } from '../core/bus.js';
import { log } from '../core/log.js';
import { e2eePool } from './e2ee-pool.js';
import { channelsStore } from '../domain/channels-store.js';

const plog = log('intent');

function connFor(channelId) {
  const conn = e2eePool.forChannel(channelId);
  if (!conn || !conn.connected) {
    plog.warn('no connected E2EE for channel', channelId);
    return null;
  }
  return conn;
}

export function bindIntentDispatcher() {
  bus.on('intent.connect_device', async ({ deviceId }) => {
    if (!deviceId) return;
    try { await e2eePool.connect(deviceId); }
    catch (err) { plog.error('connect_device failed', err); }
  });

  bus.on('intent.send_message', async ({ channelId, text, attachments, planMode }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    const content = text || '';
    const hasAttachments = Array.isArray(attachments) && attachments.length > 0;
    if (!content && !hasAttachments) return;
    // Build payload directly — matches v1 chat/composer.js wire shape.
    const payload = { action: 'message', channel_id: channelId, content };
    if (hasAttachments) payload.attachments = attachments;
    if (planMode) payload.plan_mode = true;
    const ch = channelsStore.get(channelId);
    if (ch?.model) payload.model = ch.model;
    if (ch?.effort) payload.effort = ch.effort;
    try { await conn.send(payload); }
    catch (err) { plog.error('send_message failed', err); }
  });

  bus.on('intent.stop_agent', async ({ channelId }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.stopAgent(channelId); }
    catch (err) { plog.error('stop_agent failed', err); }
  });

  bus.on('intent.restart_agent', async ({ channelId }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.restartAgent(channelId); }
    catch (err) { plog.error('restart_agent failed', err); }
  });

  bus.on('intent.update_channel', async ({ channelId, patch }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.updateChannel(channelId, patch || {}); }
    catch (err) { plog.error('update_channel failed', err); }
  });

  bus.on('intent.delete_channel', async ({ channelId }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.deleteChannel(channelId); }
    catch (err) { plog.error('delete_channel failed', err); }
  });

  bus.on('intent.rename_channel', async ({ channelId, name }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.renameChannel(channelId, name); }
    catch (err) { plog.error('rename_channel failed', err); }
  });

  bus.on('intent.mark_read', async ({ channelId, msgIds }) => {
    const conn = connFor(channelId);
    if (!conn || !msgIds?.length) return;
    try { await conn.markRead(msgIds); }
    catch (err) { plog.error('mark_read failed', err); }
  });

  bus.on('intent.mark_seen', async ({ channelId }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.markSeen(channelId); }
    catch (err) { plog.error('mark_seen failed', err); }
  });

  bus.on('intent.reset_session', async ({ channelId }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.resetSession(channelId); }
    catch (err) { plog.error('reset_session failed', err); }
  });

  bus.on('intent.compact_session', async ({ channelId }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.compactSession(channelId); }
    catch (err) { plog.error('compact_session failed', err); }
  });

  bus.on('intent.interaction_response', async ({ channelId, interactionId, selectedOption, freeformResponse, selectedOptions }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try {
      await conn.sendInteractionResponse(channelId, interactionId, selectedOption, freeformResponse, selectedOptions);
    } catch (err) { plog.error('interaction_response failed', err); }
  });

  bus.on('intent.terminal_exec', async ({ channelId, command, cwd, commandId }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.terminalExec(channelId, command, cwd, commandId); }
    catch (err) { plog.error('terminal_exec failed', err); }
  });

  bus.on('intent.terminal_kill', async ({ channelId, commandId }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.terminalKill(channelId, commandId); }
    catch (err) { plog.error('terminal_kill failed', err); }
  });

  bus.on('intent.terminal_complete', async ({ channelId, partial, line, cwd }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.terminalComplete(channelId, partial, line, cwd); }
    catch (err) { plog.error('terminal_complete failed', err); }
  });

  bus.on('intent.files_list', async ({ channelId, path }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.filesList(channelId, path || ''); }
    catch (err) { plog.error('files_list failed', err); }
  });

  bus.on('intent.files_changes', async ({ channelId }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.filesChanges(channelId); }
    catch (err) { plog.error('files_changes failed', err); }
  });

  bus.on('intent.file_read', async ({ channelId, path, offset, limit }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.fileRead(channelId, path, offset, limit); }
    catch (err) { plog.error('file_read failed', err); }
  });

  bus.on('intent.file_diff', async ({ channelId, path, staged }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.fileDiff(channelId, path, staged); }
    catch (err) { plog.error('file_diff failed', err); }
  });

  bus.on('intent.get_messages', async ({ channelId, limit, before }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.getMessages(channelId, limit, before); }
    catch (err) { plog.error('get_messages failed', err); }
  });

  bus.on('intent.get_activity', async ({ channelId }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.getActivity(channelId); }
    catch (err) { plog.error('get_activity failed', err); }
  });

  bus.on('intent.get_complications', async ({ channelId }) => {
    const conn = connFor(channelId);
    if (!conn) return;
    try { await conn.getComplications(channelId); }
    catch (err) { plog.error('get_complications failed', err); }
  });

  bus.on('intent.resolve_complication', async ({ channelId, complicationId, action }) => {
    const conn = connFor(channelId);
    if (!conn || !complicationId || !action) return;
    try {
      await conn.send({
        action: 'complication:action',
        channel_id: channelId,
        complication_id: complicationId,
        option_id: action,
      });
    } catch (err) { plog.error('resolve_complication failed', err); }
  });
}
