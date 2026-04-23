// Translates BuildE2EE wire events to domain bus events.
//
// Pure fan-out: no state, no DOM, no store imports. See
// planning/dashboard-v2/04-transport.md for vocabulary.

import { bus } from '../core/bus.js';
import { log } from '../core/log.js';

const plog = log('e2ee-dispatcher');

export function bindE2EEDispatcher(instance, deviceId) {
  if (!instance) return;

  // ----- Connection lifecycle -----
  instance.addEventListener('connected', () => {
    bus.emit('e2ee.connected', { deviceId });
  });
  instance.addEventListener('disconnected', () => {
    bus.emit('e2ee.disconnected', { deviceId });
    // Also mark the device offline so the sidebar collapses its
    // group right away — the Skrift `build:device:offline`
    // notification may lag (or never fire if the relay itself
    // missed the close). Defensive; idempotent with the
    // Skrift-driven path.
    if (deviceId) bus.emit('device.status_changed', { deviceId, status: 'offline' });
  });

  // ----- Channels -----
  instance.addEventListener('channel_list', (evt) => {
    const { channels, agent_cwd } = evt.detail;
    bus.emit('channel.list', { deviceId, channels, agentCwd: agent_cwd });
  });
  instance.addEventListener('channel_created', (evt) => {
    bus.emit('channel.upserted', { deviceId, channel: evt.detail });
  });
  instance.addEventListener('channel_renamed', (evt) => {
    const { channel_id, name } = evt.detail;
    bus.emit('channel.patched', { channelId: channel_id, patch: { name } });
  });
  instance.addEventListener('channel_updated', (evt) => {
    const { channel_id, harness, model, effort, working_directory, auto_approve_tools } = evt.detail;
    const patch = {};
    if (harness !== undefined && harness !== null) patch.harness = harness;
    if (model !== undefined && model !== null) patch.model = model;
    if (effort !== undefined && effort !== null) patch.effort = effort;
    if (working_directory !== undefined && working_directory !== null) patch.working_directory = working_directory;
    if (auto_approve_tools !== undefined && auto_approve_tools !== null) patch.auto_approve_tools = !!auto_approve_tools;
    bus.emit('channel.patched', { channelId: channel_id, patch });
  });
  instance.addEventListener('channel_deleted', (evt) => {
    bus.emit('channel.removed', { deviceId, channelId: evt.detail.channel_id });
  });

  // ----- Messages -----
  instance.addEventListener('messages', (evt) => {
    const { channel_id, messages } = evt.detail;
    bus.emit('message.bulk', { channelId: channel_id, msgs: messages });
  });
  instance.addEventListener('message', (evt) => {
    const msg = evt.detail;
    if (!msg || !msg.channel_id) return;
    bus.emit('message.received', { channelId: msg.channel_id, msg });
  });
  instance.addEventListener('delivered', (evt) => {
    bus.emit('message.delivered', { channelId: evt.detail.channel_id, msgId: evt.detail.message_id });
  });
  instance.addEventListener('read', (evt) => {
    bus.emit('message.read', { channelId: evt.detail.channel_id, msgIds: evt.detail.message_ids });
  });
  instance.addEventListener('delivery_failed', (evt) => {
    bus.emit('message.delivery_failed', { channelId: evt.detail.channel_id, msgId: evt.detail.message_id });
  });
  instance.addEventListener('system_message', (evt) => {
    bus.emit('message.system', { channelId: evt.detail.channel_id, text: evt.detail.text });
  });
  instance.addEventListener('session_reset', (evt) => {
    bus.emit('session.reset', { channelId: evt.detail.channel_id });
  });
  instance.addEventListener('compact_started', (evt) => {
    bus.emit('session.compacting', { channelId: evt.detail.channel_id });
  });

  // ----- Agent / harness -----
  instance.addEventListener('harness_list', (evt) => {
    bus.emit('harness.list', { deviceId, harnesses: evt.detail });
  });
  instance.addEventListener('agent_started', (evt) => {
    bus.emit('agent.started', { channelId: evt.detail?.channel_id });
  });
  instance.addEventListener('agent_stopped', (evt) => {
    bus.emit('agent.stopped', { channelId: evt.detail?.channel_id });
  });
  instance.addEventListener('agent_restarted', (evt) => {
    bus.emit('agent.restarted', { channelId: evt.detail?.channel_id });
  });
  instance.addEventListener('plan_mode_updated', (evt) => {
    bus.emit('agent.plan_mode', { channelId: evt.detail.channel_id, planMode: evt.detail.plan_mode });
  });
  instance.addEventListener('e2ee_error', (evt) => {
    bus.emit('e2ee.error', { deviceId, error: evt.detail });
  });

  instance.addEventListener('activity_history', (evt) => {
    const { channel_id, entries } = evt.detail;
    bus.emit('agent.activity_history', { channelId: channel_id, entries });
  });

  instance.addEventListener('agent_event', (evt) => {
    const { channel_id, event_type, event } = evt.detail ?? {};
    if (!channel_id) return;

    switch (event_type) {
      case 'chat.response': {
        const msg = {
          id: event.id || '',
          channel_id,
          sender: event.sender || 'Device',
          content: event.content || '',
          created_at: event.created_at || new Date().toISOString(),
        };
        if (event.suggested_actions?.length) msg.suggested_actions = event.suggested_actions;
        bus.emit('message.received', { channelId: channel_id, msg });
        bus.emit('agent.active', { channelId: channel_id, active: true });
        break;
      }
      case 'activity.delta': {
        const delta = event.delta || {};
        if (delta.type === 'text' && delta.text) {
          bus.emit('agent.reasoning', { channelId: channel_id, text: delta.text, at: event.created_at });
        }
        bus.emit('agent.active', { channelId: channel_id, active: true });
        break;
      }
      case 'tool.use': {
        const name = event.name || 'tool';
        const input = event.input || {};
        bus.emit('agent.tool_use', {
          channelId: channel_id,
          toolUseId: event.tool_use_id,
          name,
          input,
          at: event.created_at,
        });
        if (name === 'TodoWrite' && input.todos) {
          bus.emit('agent.todo_write', { channelId: channel_id, todos: input.todos });
        }
        bus.emit('agent.active', { channelId: channel_id, active: true });
        break;
      }
      case 'tool.result':
        bus.emit('agent.tool_result', {
          channelId: channel_id,
          toolUseId: event.tool_use_id,
          isError: !!event.is_error,
          content: event.content,
          at: event.completed_at,
        });
        break;
      case 'activity.end':
        bus.emit('agent.activity_end', { channelId: channel_id });
        bus.emit('agent.active', { channelId: channel_id, active: false });
        break;
      case 'interaction.request': {
        const interactionId = event.interaction_id;
        const kind = event.kind || 'question';
        bus.emit('interaction.requested', {
          channelId: channel_id,
          interactionId,
          kind,
          question: event.question || '',
          options: event.options || [],
          allowFreeform: event.allow_freeform !== false,
          plan: event.plan || null,
          multiselect: !!event.multiselect,
          questions: event.questions || null,
        });
        // Mirror as a message for chat display (v1 parity; views choose rendering).
        const msg = {
          id: interactionId || '',
          channel_id,
          sender: event.sender || 'Device',
          content: event.question || '',
          created_at: new Date().toISOString(),
          metadata: JSON.stringify({
            interaction_id: interactionId,
            kind,
            options: event.options || [],
            allow_freeform: event.allow_freeform !== false,
            plan: event.plan || null,
            multiselect: !!event.multiselect,
            questions: event.questions || null,
          }),
        };
        bus.emit('message.received', { channelId: channel_id, msg });
        break;
      }
      case 'agent.error':
        bus.emit('agent.error', {
          channelId: channel_id,
          message: event.message || '',
          fatal: !!event.fatal,
        });
        if (event.fatal) bus.emit('agent.active', { channelId: channel_id, active: false });
        break;
      case 'agent.state_update':
        bus.emit('agent.state_update', { channelId: channel_id, patch: event });
        if (event.plan_mode != null) {
          bus.emit('agent.plan_mode', { channelId: channel_id, planMode: event.plan_mode });
        }
        // Read receipts from the agent ride along with state updates
        // (read_unread MCP tool → BAP state_update with
        // `read_message_ids`). Fan out as message.read so the sender
        // UI's "Delivered" → "Read" flip happens in real time instead
        // of only on reload.
        if (Array.isArray(event.read_message_ids) && event.read_message_ids.length) {
          bus.emit('message.read', { channelId: channel_id, msgIds: event.read_message_ids });
        }
        break;
      case 'agent.file_changes':
        bus.emit('agent.file_changes', { channelId: channel_id, paths: event.paths ?? [] });
        break;
      default:
        plog.debug('unknown agent_event type', event_type);
    }
  });

  // ----- Terminal -----
  instance.addEventListener('terminal_output', (evt) => {
    const { channel_id, data, done, exit_code, cwd } = evt.detail;
    if (!channel_id) return;
    if (data && !done) {
      // The bridge wraps every command to append a `__BUILD_CWD__<pwd>`
      // sentinel line so it can extract the resulting cwd. That sentinel
      // is internal plumbing — strip it from the user-visible scrollback.
      const cleaned = data.replace(/^__BUILD_CWD__[^\n]*\n?/gm, '');
      if (cleaned) bus.emit('terminal.output', { channelId: channel_id, text: cleaned });
    }
    if (done) {
      bus.emit('terminal.complete', { channelId: channel_id, exitCode: exit_code, cwd });
    }
  });
  instance.addEventListener('terminal_completions', (evt) => {
    bus.emit('terminal.completions', {
      channelId: evt.detail.channel_id,
      candidates: evt.detail.completions ?? [],
    });
  });

  // ----- Files -----
  instance.addEventListener('files_list_result', (evt) => {
    const d = evt.detail;
    bus.emit('files.list_result', {
      channelId: d.channel_id,
      path: d.path || '',
      entries: d.entries || [],
      truncated: !!d.truncated,
      error: d.error,
    });
  });
  instance.addEventListener('files_changes_result', (evt) => {
    const d = evt.detail;
    bus.emit('files.changes_result', {
      channelId: d.channel_id,
      repos: d.repos || [],
      error: d.error,
    });
  });
  instance.addEventListener('file_read_result', (evt) => {
    bus.emit('files.read_result', { ...evt.detail });
  });
  instance.addEventListener('file_diff_result', (evt) => {
    bus.emit('files.diff_result', { ...evt.detail });
  });

  // ----- Uploads -----
  instance.addEventListener('upload_progress', (evt) => {
    const d = evt.detail;
    bus.emit('upload.progress', {
      deviceId,
      fileId: d.file_id,
      fileName: d.filename,
      progress: d.progress,
      totalChunks: d.total_chunks,
      chunksDone: d.chunks_done,
    });
  });
  instance.addEventListener('upload_done', (evt) => {
    const d = evt.detail;
    bus.emit('upload.done', {
      deviceId,
      fileId: d.file_id,
      fileName: d.filename,
      size: d.size,
    });
  });

  // ----- Complications -----
  instance.addEventListener('complication_update', (evt) => {
    const comp = evt.detail;
    if (!comp?.channel_id) return;
    bus.emit('complication.upserted', { channelId: comp.channel_id, complication: comp });
  });
  instance.addEventListener('complication_remove', (evt) => {
    bus.emit('complication.removed', {
      channelId: evt.detail.channel_id,
      complicationId: evt.detail.id,
    });
  });
  instance.addEventListener('complications', (evt) => {
    const { channel_id, complications } = evt.detail;
    bus.emit('complications.bulk', { channelId: channel_id, complications: complications || [] });
  });
}
