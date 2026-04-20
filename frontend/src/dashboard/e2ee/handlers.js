import { state } from '../state.js';
import { escapeHtml } from '../util/html.js';
import { agentShortName, describeToolUse, formatToolDetail, formatToolResult } from '../console/tools.js';
import { getE2EE, anyE2EEConnected } from './bridge.js';
import { syncE2EEStatus } from './connect.js';
import {
  rebuildChatChannels,
  promoteChannel,
  renderChannelList,
  renderChannelSidebar,
  incrementUnread,
} from '../channels/list.js';
import { setLastSeen, getLastSeen, deferMarkRead } from '../channels/unread.js';
import { renderChannelPanel } from '../channels/panel.js';
import { selectChannel } from '../channels/select.js';
import { applyChannelHarnessInfo, syncChatOverlayHeader } from '../chat/overlay.js';
import { renderMessages, appendMessage, appendSystemMessage, updateStopButton } from '../chat/messages.js';
import {
  appendInteractionCard,
  resolvePendingPlanReviews,
  crossfadeStatus,
  updatePlanModeUI,
} from '../chat/interactions.js';
import {
  isChatNearBottom,
  isConsoleNearBottom,
  scrollChatToBottom,
  handleNewMessageScroll,
  hideChatBubble,
  hideActivityBubble,
  scrollToFirstUnread,
  appendConsoleReasoning,
  appendConsoleEntry,
  markConsoleEntryDone,
  clearConsole,
} from '../console/view.js';
import { renderTerminalCwd, renderTerminalForChannel, finishTerminalCommand, clearTerminalTimers, clearTerminalCompletions } from '../terminal/terminal.js';
import { renderComplications } from '../complications/render.js';
import { renderTasksPanel, updateTasksBadge } from '../tasks/panel.js';
import { _bindUploadProgress } from '../chat/uploads.js';
import { fileContentBody } from '../files/refs.js';
import { renderFileContent } from '../files/content.js';
import { renderDiffContent } from '../files/diff.js';
import { renderFileTree, selectFile, onAgentFileChanges } from '../files/tree.js';
import { updateFilesModifiedCount, onFilesTabActivated } from '../files/mode.js';
import { formatBytes } from '../util/html.js';

export function bindE2EEEvents(instance, deviceId) {
  if (!instance) return;
  instance.addEventListener('connected', () => {
    console.log('[E2EE] Session established for device', deviceId);
    state.e2eeHasConnected = true;
    const _enableBtns = ['chat-input', 'chat-send-btn', 'cmd-attach-btn', 'cmd-plan-btn', 'cmd-compact-btn', 'cmd-reset-btn'];
    for (const id of _enableBtns) {
      const el = document.getElementById(id);
      if (el) el.disabled = false;
    }
    document.getElementById('e2ee-waiting-overlay').classList.add('hidden');
    // Hide reconnect pill + cancel timer.
    clearTimeout(state._reconnectPillTimer);
    state._reconnectPillTimer = null;
    document.getElementById('reconnect-pill').classList.remove('visible');
    // Clear device-down banner on successful reconnect.
    state.deviceDown = null;
    const _ddb = document.getElementById('device-down-banner');
    if (_ddb) _ddb.classList.remove('visible');
    if (typeof _bindUploadProgress === 'function') _bindUploadProgress(instance);
    syncE2EEStatus();
    renderChannelPanel();
    instance.listChannels();
    instance.listHarnesses();
    // Re-fetch history for the current channel if it belongs to this device.
    if (state.chatCurrentChannel && state.channelDeviceMap.get(state.chatCurrentChannel) === deviceId) {
      state.chatLoadingMessages.add(state.chatCurrentChannel);
      state.chatLoadingActivity.add(state.chatCurrentChannel);
      instance.getMessages(state.chatCurrentChannel);
      instance.getActivity(state.chatCurrentChannel);
      // If the files tab is active, retry the tree/changes fetch — the initial
      // selectChannel() may have run before this `connected` flag flipped.
      if (state.currentTab === 'files') {
        state.filesChannelId = null;
        if (typeof onFilesTabActivated === 'function') onFilesTabActivated();
      }
    }
  });

  instance.addEventListener('disconnected', () => {
    console.log('[E2EE] Disconnected device', deviceId);
    state.e2eeConnections.delete(deviceId);
    // Keep state.deviceChannels and state.channelDeviceMap cached — only clear on server-reported removal.
    state.deviceHarnesses.delete(deviceId);
    state.deviceAgentCwd.delete(deviceId);

    if (!anyE2EEConnected()) {
      const _disableBtns = ['chat-input', 'chat-send-btn', 'cmd-attach-btn', 'cmd-plan-btn', 'cmd-compact-btn', 'cmd-reset-btn'];
      for (const id of _disableBtns) {
        const el = document.getElementById(id);
        if (el) el.disabled = true;
      }
      if (state.deviceDown) {
        // Device is known to be offline — keep banner visible, skip skeleton/pill.
      } else if (!state.e2eeHasConnected) {
        // First load — show skeleton.
        document.getElementById('e2ee-waiting-overlay').classList.remove('hidden');
      } else {
        // Reconnection — show pill after 2s delay.
        clearTimeout(state._reconnectPillTimer);
        state._reconnectPillTimer = setTimeout(() => {
          if (!anyE2EEConnected()) {
            document.getElementById('reconnect-pill').classList.add('visible');
          }
        }, 2000);
      }
    }
    syncE2EEStatus();
    renderChannelPanel();
  });

  // ----- Channel list -----

  instance.addEventListener('channel_list', (evt) => {
    const { channels, agent_cwd } = evt.detail;
    if (agent_cwd) state.deviceAgentCwd.set(deviceId, agent_cwd);
    // Diff against cached channels to detect removals.
    const oldChans = state.deviceChannels.get(deviceId);
    const oldIds = oldChans ? new Set(oldChans.keys()) : new Set();
    // Update per-device channel map.
    const devChans = new Map();
    for (const ch of channels) {
      devChans.set(ch.id, ch);
      state.channelDeviceMap.set(ch.id, deviceId);
      if (ch.plan_mode != null) state.channelPlanMode.set(ch.id, ch.plan_mode);
      if (ch.last_seen_at) state.channelLastSeen.set(ch.id, ch.last_seen_at);
      // Reset agent active state — real-time events will re-set if agent is mid-turn.
      state.channelAgentActive.set(ch.id, false);
    }
    updateStopButton();
    state.deviceChannels.set(deviceId, devChans);
    // Clean up channels removed by the device.
    for (const oldId of oldIds) {
      if (!devChans.has(oldId)) {
        state.channelDeviceMap.delete(oldId);
        state.chatMessages.delete(oldId);
        state.unreadCounts.delete(oldId);
        state.channelSortTs.delete(oldId);
      }
    }
    rebuildChatChannels();
    renderChannelList();
    // If current channel was removed by the device, auto-select another.
    if (state.chatCurrentChannel && oldIds.has(state.chatCurrentChannel) && !devChans.has(state.chatCurrentChannel)) {
      const remaining = [...state.chatChannels.values()];
      if (remaining.length > 0) selectChannel(remaining[0].id);
      else { state.chatCurrentChannel = null; renderMessages(); }
    }
    // Restore pending channel from hash.
    if (state._pendingChannelId && state.chatChannels.has(state._pendingChannelId)) {
      selectChannel(state._pendingChannelId);
      state._pendingChannelId = null;
    } else if (!state.chatCurrentChannel && !state._pendingChannelId && channels.length > 0) {
      // Only auto-select first channel if there's no pending channel waiting for another device.
      selectChannel(channels[0].id);
    }
    // Fetch messages for all non-current channels to compute unread counts.
    for (const ch of channels) {
      if (ch.id !== state.chatCurrentChannel && instance.connected) {
        instance.getMessages(ch.id);
      }
    }
  });

  instance.addEventListener('channel_created', (evt) => {
    const ch = evt.detail;
    state.channelDeviceMap.set(ch.id, deviceId);
    const devChans = state.deviceChannels.get(deviceId) || new Map();
    devChans.set(ch.id, ch);
    state.deviceChannels.set(deviceId, devChans);
    rebuildChatChannels();
    renderChannelList();
    selectChannel(ch.id);
  });

  // ----- Harness & Agent events -----

  instance.addEventListener('harness_list', (evt) => {
    state.deviceHarnesses.set(deviceId, evt.detail);
    console.log('[E2EE] Harnesses for', deviceId, ':', evt.detail.map(h => h.name));
  });

  instance.addEventListener('agent_started', (evt) => {
    console.log('[E2EE] Agent started:', evt.detail);
  });

  instance.addEventListener('agent_stopped', (evt) => {
    console.log('[E2EE] Agent stopped:', evt.detail);
    const ch = evt.detail?.channel_id;
    if (ch) {
      state.channelAgentActive.set(ch, false);
      updateStopButton();
    }
  });

  instance.addEventListener('agent_restarted', (evt) => {
    console.log('[E2EE] Agent restarted:', evt.detail);
  });

  instance.addEventListener('channel_renamed', (evt) => {
    const { channel_id, name } = evt.detail;
    const ch = state.chatChannels.get(channel_id);
    if (ch) {
      ch.name = name;
      const devChans = state.deviceChannels.get(deviceId);
      if (devChans?.has(channel_id)) devChans.get(channel_id).name = name;
      renderChannelList();
      renderChannelSidebar();
    }
  });

  instance.addEventListener('channel_updated', (evt) => {
    const { channel_id, model, effort, working_directory } = evt.detail;
    const ch = state.chatChannels.get(channel_id);
    if (ch) {
      if (model) ch.model = model;
      if (effort !== undefined) ch.effort = effort;
      if (working_directory !== undefined) ch.working_directory = working_directory;
      const devChans = state.deviceChannels.get(deviceId);
      if (devChans?.has(channel_id)) {
        const dc = devChans.get(channel_id);
        if (model) dc.model = model;
        if (effort !== undefined) dc.effort = effort;
        if (working_directory !== undefined) dc.working_directory = working_directory;
      }
    }
    // If this is the active channel, refresh the chat overlay model/effort pills.
    if (state.chatCurrentChannel === channel_id) applyChannelHarnessInfo(channel_id);
  });

  instance.addEventListener('channel_deleted', (evt) => {
    const { channel_id } = evt.detail;
    state.channelDeviceMap.delete(channel_id);
    const devChans = state.deviceChannels.get(deviceId);
    if (devChans) devChans.delete(channel_id);
    rebuildChatChannels();
    state.unreadCounts.delete(channel_id);
    if (state.chatCurrentChannel === channel_id) {
      const remaining = [...state.chatChannels.values()];
      if (remaining.length > 0) {
        selectChannel(remaining[0].id);
      } else {
        state.chatCurrentChannel = null;
        renderMessages();
      }
    }
    renderChannelList();
    renderChannelSidebar();
  });

  instance.addEventListener('e2ee_error', (evt) => {
    console.error('[E2EE] Error:', evt.detail);
  });

// ----- Messages -----

  instance.addEventListener('messages', (evt) => {
    const { channel_id, messages } = evt.detail;
    state.chatLoadingMessages.delete(channel_id);
    state.chatMessages.set(channel_id, messages);
    // Seed sort timestamp from newest message on initial load (don't override live promotions).
    if (!state.channelSortTs.has(channel_id) && messages.length) {
      const newest = messages[messages.length - 1];
      const ts = typeof newest.created_at === 'number' ? newest.created_at * 1000 : Date.parse(newest.created_at);
      if (ts) state.channelSortTs.set(channel_id, ts);
    }
    if (state.chatCurrentChannel === channel_id) {
      renderMessages();
      // Defer marking unread device messages as read until user interacts.
      const unread = messages
        .filter(m => m.sender !== 'client' && !m.read_at)
        .map(m => m.id)
        .filter(Boolean);
      if (unread.length && instance.connected) {
        const _inst = instance;
        deferMarkRead(() => _inst.markRead(unread));
      }
    } else {
      // Compute unread count for background channels using lastSeen timestamp.
      const lastSeen = getLastSeen(channel_id);
      const unreadMsgs = messages.filter(m => {
        if (m.sender === 'client') return false;
        if (!lastSeen) return true; // Never seen — all agent messages are unread.
        const msgTime = typeof m.created_at === 'number'
          ? new Date(m.created_at * 1000).toISOString()
          : m.created_at;
        return msgTime > lastSeen;
      });
      const hasInteraction = unreadMsgs.some(m => {
        try {
          const meta = typeof m.metadata === 'string' ? JSON.parse(m.metadata) : m.metadata;
          return meta && meta.interaction_id && !meta.resolved_at;
        } catch { return false; }
      });
      if (unreadMsgs.length > 0 || hasInteraction) {
        state.unreadCounts.set(channel_id, { messages: unreadMsgs.length, hasInteraction });
      } else {
        state.unreadCounts.delete(channel_id);
      }
      renderChannelList();
    }
  });

  instance.addEventListener('message', (evt) => {
    const msg = evt.detail;
    if (!msg || !msg.channel_id) return;
    const msgs = state.chatMessages.get(msg.channel_id) || [];
    // Dedup: skip if message with same ID already exists.
    if (msg.id && msgs.some(m => m.id === msg.id)) return;
    msgs.push(msg);
    state.chatMessages.set(msg.channel_id, msgs);
    promoteChannel(msg.channel_id);
    if (state.chatCurrentChannel === msg.channel_id) {
      const wasNearBottom = isChatNearBottom();
      appendMessage(msg);
      const _newEl = document.getElementById('chat-messages').lastElementChild;
      handleNewMessageScroll(wasNearBottom, _newEl);
      const _msgChId = msg.channel_id;
      const _msgId = msg.id;
      const _inst = instance;
      deferMarkRead(() => {
        setLastSeen(_msgChId);
        if (_msgId) _inst.markRead([_msgId]);
      });
    } else if (msg.sender !== 'client') {
      incrementUnread(msg.channel_id);
    }
  });

  // Agent events (chat.response, activity.delta, tool.use, etc.) from spawned agents.
  instance.addEventListener('agent_event', (evt) => {
    const { channel_id, event_type, event: agentEvt } = evt.detail;
    if (!channel_id) return;

    if (event_type === 'chat.response') {
      // Agent sent a chat message — also means agent is active.
      if (!state.channelAgentActive.get(channel_id)) {
        state.channelAgentActive.set(channel_id, true);
        updateStopButton();
      }
      promoteChannel(channel_id);
      const msg = {
        id: agentEvt.id || '',
        channel_id,
        sender: agentEvt.sender || 'Device',
        content: agentEvt.content || '',
        created_at: new Date().toISOString(),
      };
      if (agentEvt.suggested_actions?.length) msg.suggested_actions = agentEvt.suggested_actions;
      const msgs = state.chatMessages.get(channel_id) || [];
      if (msg.id && msgs.some(m => m.id === msg.id)) return;
      msgs.push(msg);
      state.chatMessages.set(channel_id, msgs);
      if (state.chatCurrentChannel === channel_id) {
        const wasNearBottom = isChatNearBottom();
        appendMessage(msg);
        const _newEl = document.getElementById('chat-messages').lastElementChild;
        handleNewMessageScroll(wasNearBottom, _newEl);
        const _agChId = channel_id;
        deferMarkRead(() => setLastSeen(_agChId));
      } else {
        incrementUnread(channel_id);
      }
    } else if (event_type === 'activity.delta') {
      if (!state.channelAgentActive.get(channel_id)) {
        state.channelAgentActive.set(channel_id, true);
        updateStopButton();
      }
      if (state.chatCurrentChannel !== channel_id) return;
      const delta = agentEvt.delta || {};
      if (delta.type === 'text' && delta.text) {
        appendConsoleReasoning(delta.text, agentEvt.created_at || null);
      }
    } else if (event_type === 'tool.use') {
      if (!state.channelAgentActive.get(channel_id)) {
        state.channelAgentActive.set(channel_id, true);
        updateStopButton();
      }
      // Capture TodoWrite for any channel (before early return)
      const name = agentEvt.name || 'tool';
      const input = agentEvt.input || {};
      if (name === 'TodoWrite' && input.todos) {
        state.channelTodos.set(channel_id, input.todos);
        if (state.chatCurrentChannel === channel_id) {
          renderTasksPanel(channel_id);
          updateTasksBadge(channel_id);
        }
      }
      if (state.chatCurrentChannel !== channel_id) return;
      currentReasoningEntry = null;
      const desc = describeToolUse(name, input);
      appendConsoleEntry(agentEvt.tool_use_id, name, desc, input, agentEvt.created_at);
    } else if (event_type === 'tool.result') {
      if (state.chatCurrentChannel !== channel_id) return;
      markConsoleEntryDone(agentEvt.tool_use_id, agentEvt.is_error, agentEvt.content, agentEvt.completed_at);
    } else if (event_type === 'activity.end') {
      state.channelAgentActive.set(channel_id, false);
      updateStopButton();
      if (state.chatCurrentChannel !== channel_id) return;
    } else if (event_type === 'interaction.request') {
      // Agent is asking the user a question — render inline in chat.
      promoteChannel(channel_id);
      const msg = {
        id: agentEvt.interaction_id || '',
        channel_id,
        sender: agentEvt.sender || 'Device',
        content: agentEvt.question || '',
        created_at: new Date().toISOString(),
        metadata: JSON.stringify({
          interaction_id: agentEvt.interaction_id,
          kind: agentEvt.kind || 'question',
          options: agentEvt.options || [],
          allow_freeform: agentEvt.allow_freeform !== false,
          plan: agentEvt.plan || null,
          multiselect: !!agentEvt.multiselect,
        }),
      };
      const msgs = state.chatMessages.get(channel_id) || [];
      msgs.push(msg);
      state.chatMessages.set(channel_id, msgs);
      if (state.chatCurrentChannel === channel_id) {
        const wasNearBottom = isChatNearBottom();
        appendMessage(msg);
        const _newEl = document.getElementById('chat-messages').lastElementChild;
        handleNewMessageScroll(wasNearBottom, _newEl);
        // On mobile, collapse the console to give more room for plan review cards,
        // then scroll the plan card to the top of the chat area.
        if ((agentEvt.kind || 'question') === 'plan_review' && window.innerWidth <= 768 && state.consoleState !== 'collapsed') {
          setConsoleState('collapsed');
          requestAnimationFrame(() => {
            if (_newEl) _newEl.scrollIntoView({ block: 'start', behavior: 'instant' });
          });
        }
      } else {
        incrementUnread(channel_id, true);
      }
    } else if (event_type === 'agent.error') {
      // Display agent errors as system messages in chat.
      const errMsg = {
        id: 'err-' + Date.now(),
        channel_id,
        sender: 'system',
        content: `**Agent error** — ${agentEvt.message || 'Unknown error'}`,
        created_at: new Date().toISOString(),
      };
      const msgs = state.chatMessages.get(channel_id) || [];
      msgs.push(errMsg);
      state.chatMessages.set(channel_id, msgs);
      if (state.chatCurrentChannel === channel_id) {
        const wasNearBottom = isChatNearBottom();
        appendMessage(errMsg);
        const _newEl = document.getElementById('chat-messages').lastElementChild;
        handleNewMessageScroll(wasNearBottom, _newEl);
      } else {
        incrementUnread(channel_id, true);
      }
      if (agentEvt.fatal) {
        state.channelAgentActive.set(channel_id, false);
        updateStopButton();
      }
    } else if (event_type === 'agent.state_update') {
      const planMode = agentEvt.plan_mode;
      if (planMode != null) {
        state.channelPlanMode.set(channel_id, planMode);
        if (channel_id === state.chatCurrentChannel) updatePlanModeUI(planMode);
      }
      // Handle read notifications from agent.
      const readIds = agentEvt.read_message_ids;
      if (readIds && readIds.length) {
        for (const mid of readIds) {
          const statusEl = document.querySelector(`[data-msg-id="${mid}"] .msg-status`);
          if (statusEl) {
            crossfadeStatus(statusEl, 'read', '<span class="check active">✓</span><span class="check active">✓</span> Read');
          }
        }
      }
    } else if (event_type === 'agent.file_changes') {
      onAgentFileChanges(channel_id, agentEvt?.paths || []);
    }
  });

  // Activity history — tool use console entries loaded on channel select / reconnect.
  instance.addEventListener('activity_history', (evt) => {
    const { channel_id, entries } = evt.detail;
    state.chatLoadingActivity.delete(channel_id);
    if (state.chatCurrentChannel !== channel_id) return;

    clearConsole();
    for (const entry of entries) {
      if (entry.type === 'tool_use') {
        const d = entry.data || {};
        const name = d.name || 'tool';
        const input = d.input || {};
        // Capture TodoWrite from history (last one wins)
        if (name === 'TodoWrite' && input.todos) {
          state.channelTodos.set(channel_id, input.todos);
        }
        const desc = describeToolUse(name, input);
        appendConsoleEntry(d.id || '', name, desc, input, entry.created_at);
      } else if (entry.type === 'tool_result') {
        const d = entry.data || {};
        markConsoleEntryDone(d.tool_use_id || '', d.is_error || false, d.content, entry.created_at);
      } else if (entry.type === 'text') {
        const content = (entry.data?.text) || '';
        if (content) {
          currentReasoningEntry = null;
          appendConsoleReasoning(content, entry.created_at);
          currentReasoningEntry = null;
        }
      }
    }
    // Render tasks if we captured any TodoWrite calls from history.
    if (state.channelTodos.has(channel_id)) {
      renderTasksPanel(channel_id);
      updateTasksBadge(channel_id);
    }
    // Scroll to bottom so latest tool use is visible.
    const body = document.querySelector('[data-console-panel="activity"]');
    if (body) body.scrollTop = body.scrollHeight;
  });

  instance.addEventListener('delivered', (evt) => {
    const { message_id } = evt.detail;
    const statusEl = document.querySelector(`[data-msg-id="${message_id}"] .msg-status`);
    if (statusEl) {
      crossfadeStatus(statusEl, 'delivered', '<span class="check active">✓</span><span class="check">✓</span> Delivered');
    }
  });

  instance.addEventListener('read', (evt) => {
    const { message_ids } = evt.detail;
    for (const mid of (message_ids || [])) {
      const statusEl = document.querySelector(`[data-msg-id="${mid}"] .msg-status`);
      if (statusEl) {
        crossfadeStatus(statusEl, 'read', '<span class="check active">✓</span><span class="check active">✓</span> Read');
      }
    }
  });
  instance.addEventListener('delivery_failed', (evt) => {
    const { message_id, channel_id } = evt.detail;
    const statusEl = document.querySelector(`[data-msg-id="${message_id}"] .msg-status`);
    if (statusEl) {
      const retryHtml = `<button class="retry-btn" data-retry-msg="${message_id}" data-retry-ch="${channel_id}">Failed to reach agent — tap to retry</button>`;
      crossfadeStatus(statusEl, 'failed', retryHtml);
      statusEl.querySelector('.retry-btn')?.addEventListener('click', async () => {
        const msgEl = document.querySelector(`[data-msg-id="${message_id}"]`);
        const content = msgEl?.querySelector('.msg-text')?.textContent?.trim();
        if (!content || !instance.connected) return;
        crossfadeStatus(statusEl, 'sending', '<span class="check">✓</span><span class="check">✓</span> Sending');
        try {
          await instance.send({ action: 'retry_message', channel_id, message_id });
        } catch (err) {
          console.error('[Chat] Retry failed:', err);
          crossfadeStatus(statusEl, 'failed', retryHtml);
        }
      });
    }
  });

  instance.addEventListener('plan_mode_updated', (evt) => {
    const { channel_id, plan_mode } = evt.detail;
    state.channelPlanMode.set(channel_id, plan_mode);
    if (channel_id === state.chatCurrentChannel) updatePlanModeUI(plan_mode);
  });

  instance.addEventListener('system_message', (evt) => {
    const { channel_id, text } = evt.detail;
    if (state.chatCurrentChannel === channel_id) {
      appendSystemMessage(text);
      scrollChatToBottom();
    }
  });

  instance.addEventListener('session_reset', (evt) => {
    const { channel_id } = evt.detail;
    if (state.chatCurrentChannel === channel_id) {
      // Remove any "Compacting session..." indicator.
      const compacting = document.getElementById('compact-indicator');
      if (compacting) compacting.remove();

      const messagesEl = document.getElementById('chat-messages');
      const divider = document.createElement('div');
      divider.className = 'session-divider';
      divider.textContent = 'New session started';
      messagesEl.appendChild(divider);
      messagesEl.scrollTop = messagesEl.scrollHeight;
    }
  });

  instance.addEventListener('compact_started', (evt) => {
    const { channel_id } = evt.detail;
    if (state.chatCurrentChannel === channel_id) {
      const messagesEl = document.getElementById('chat-messages');
      const indicator = document.createElement('div');
      indicator.id = 'compact-indicator';
      indicator.className = 'session-divider';
      indicator.textContent = 'Compacting session...';
      messagesEl.appendChild(indicator);
      messagesEl.scrollTop = messagesEl.scrollHeight;
    }
  });

  // ----- Complication events -----
  instance.addEventListener('complication_update', (evt) => {
    const comp = evt.detail;
    const channelId = comp.channel_id;
    if (!channelId) return;
    if (!state.complicationState.has(channelId)) state.complicationState.set(channelId, new Map());
    state.complicationState.get(channelId).set(comp.id, comp);
    if (state.chatCurrentChannel === channelId) renderComplications();
  });

  instance.addEventListener('complication_remove', (evt) => {
    const { channel_id, id } = evt.detail;
    const channelComps = state.complicationState.get(channel_id);
    if (channelComps) {
      channelComps.delete(id);
      if (state.chatCurrentChannel === channel_id) renderComplications();
    }
  });

  instance.addEventListener('complications', (evt) => {
    const { channel_id, complications } = evt.detail;
    if (!channel_id || !complications) return;
    if (!state.complicationState.has(channel_id)) state.complicationState.set(channel_id, new Map());
    const channelComps = state.complicationState.get(channel_id);
    for (const comp of complications) {
      channelComps.set(comp.id, comp);
    }
    if (state.chatCurrentChannel === channel_id) renderComplications();
  });

  // ----- Terminal Output -----
  instance.addEventListener('terminal_output', (evt) => {
    const { channel_id, data, done, exit_code, cwd } = evt.detail;
    if (!channel_id) return;

    const sentinel = '__BUILD_CWD__';

    if (!done && data) {
      // First output received — clear loading animation and reset no-output timer.
      if (state.terminalCurrentBlock && !state.terminalCurrentBlock.hasOutput) {
        state.terminalCurrentBlock.hasOutput = true;
        clearTerminalTimers();
      }

      // Filter out sentinel line from streaming output.
      let text = data;
      if (text.includes(sentinel)) {
        text = text.split('\n').filter(l => !l.startsWith(sentinel)).join('\n');
        if (!text) return;
      }

      // Append to current streaming block if matching channel.
      if (state.terminalCurrentBlock && state.terminalCurrentBlock.channelId === channel_id) {
        state.terminalCurrentBlock.text += text;
        const span = document.createElement('span');
        span.textContent = text;
        state.terminalCurrentBlock.outputDiv.appendChild(span);
        const output = document.getElementById('terminal-output');
        output.scrollTop = output.scrollHeight;
      }
      // Update stored history.
      const history = state.terminalHistoryMap.get(channel_id);
      if (history?.length) history[history.length - 1].output += text;
    }

    if (done) {
      clearTerminalTimers();
      document.getElementById('terminal-kill-btn')?.classList.add('hidden');
      // Update cwd.
      if (cwd) {
        state.terminalCwdMap.set(channel_id, cwd);
        if (state.chatCurrentChannel === channel_id) renderTerminalCwd();
      }
      // Update stored exit code.
      const history = state.terminalHistoryMap.get(channel_id);
      if (history?.length) history[history.length - 1].exitCode = exit_code;
      // Show exit code if non-zero.
      if (state.terminalCurrentBlock && state.terminalCurrentBlock.channelId === channel_id && exit_code !== 0) {
        const exitDiv = document.createElement('div');
        exitDiv.className = 'terminal-cmd-exit error';
        exitDiv.textContent = `exit ${exit_code}`;
        state.terminalCurrentBlock.block.appendChild(exitDiv);
      }
      if (state.terminalCurrentBlock?.channelId === channel_id) {
        state.terminalCurrentBlock = null;
      }
      state.terminalRunning = false;
      // Show prompt row again with updated cwd.
      if (state.chatCurrentChannel === channel_id) {
        const promptRow = document.getElementById('terminal-prompt-row');
        promptRow.classList.remove('hidden');
        const output = document.getElementById('terminal-output');
        output.scrollTop = output.scrollHeight;
        document.getElementById('terminal-input')?.focus();
      }
    }
  });

  instance.addEventListener('terminal_completions', (evt) => {
    state.terminalCompletionPending = false;
    const { completions } = evt.detail;
    if (!completions || !completions.length) return;
    const input = document.getElementById('terminal-input');
    if (!input) return;
    // Use the context saved at request time (not the echoed partial)
    const beforePartial = state.terminalCompletionBase;
    const partial = state.terminalCompletionPartial;

    // Remove any previous completion display
    document.querySelectorAll('.terminal-completions').forEach(el => el.remove());

    if (completions.length === 1) {
      // Single match — substitute it in
      const match = completions[0];
      const suffix = match.endsWith('/') ? '' : ' ';
      input.value = beforePartial + match + suffix;
      clearTerminalCompletions();
    } else {
      // Multiple matches — find common prefix and complete that
      let common = completions[0];
      for (let i = 1; i < completions.length; i++) {
        while (common && !completions[i].startsWith(common)) {
          common = common.slice(0, -1);
        }
      }
      if (common.length > partial.length) {
        input.value = beforePartial + common;
      }
      // Store for Tab cycling
      state.terminalCompletions = completions;
      state.terminalCompletionIndex = -1;
      // Show candidates below the prompt
      const output = document.getElementById('terminal-output');
      if (output) {
        const compDiv = document.createElement('div');
        compDiv.className = 'terminal-completions';
        compDiv.textContent = completions.map(c => c.split('/').filter(Boolean).pop() + (c.endsWith('/') ? '/' : '')).join('  ');
        output.appendChild(compDiv);
        output.scrollTop = output.scrollHeight;
      }
    }
  });

  // ----- Files view events -----

  instance.addEventListener('files_list_result', (evt) => {
    const { channel_id, path, entries, error, truncated } = evt.detail;
    if (error) { console.warn('files_list error:', error); return; }
    if (channel_id !== state.filesChannelId) return;

    if (!state.fileTreeData.has(channel_id)) state.fileTreeData.set(channel_id, new Map());
    const data = state.fileTreeData.get(channel_id);
    data.set(path || '', { entries: entries || [], truncated: !!truncated });
    renderFileTree();

    // Restore saved file selection after root listing loads.
    if (state.filesPendingRestore && (path || '') === '') {
      const pendingPath = state.filesPendingRestore;
      const pendingView = state.filesPendingView;
      state.filesPendingRestore = null;
      state.filesPendingView = 'source';
      // Find the entry in the root listing.
      const match = (entries || []).find(e => e.name === pendingPath || e.path === pendingPath);
      if (match && match.type !== 'directory') {
        selectFile(match.path || match.name, match, pendingView);
      } else if (pendingPath.includes('/')) {
        // Nested path — select directly (tree won't highlight but file loads).
        selectFile(pendingPath, null, pendingView);
      }
    }
  });

  instance.addEventListener('files_changes_result', (evt) => {
    const { channel_id, repos } = evt.detail;
    console.debug('[files] files_changes_result', { channel_id, repos_count: repos?.length, filesChannelId: state.filesChannelId, chatCurrentChannel: state.chatCurrentChannel });
    // Accept if it matches state.filesChannelId, OR if state.filesChannelId is unset
    // but this is the currently-active chat channel (self-heal race).
    if (state.filesChannelId) {
      if (channel_id !== state.filesChannelId) return;
    } else if (channel_id !== state.chatCurrentChannel) {
      return;
    } else {
      state.filesChannelId = channel_id;
    }
    state.filesChangesData.set(channel_id, repos || []);
    updateFilesModifiedCount();
    if (state.filesTreeTab === 'changes') renderFileTree();
  });

  let _imageChunks = {};  // path -> { chunks: [], total: N }

  instance.addEventListener('file_read_result', (evt) => {
    const d = evt.detail;
    if (d.channel_id !== state.filesChannelId || d.path !== state.filesCurrentPath) return;
    if (state.filesCurrentView === 'diff') return;

    if (d.error) {
      fileContentBody.innerHTML = `<div class="empty-state"><p>${escapeHtml(d.error)}</p></div>`;
      return;
    }
    if (d.is_image && d.content) {
      // Handle chunked images.
      if (d.chunk_total && d.chunk_total > 1) {
        if (!_imageChunks[d.path] || _imageChunks[d.path].total !== d.chunk_total) {
          _imageChunks[d.path] = { chunks: new Array(d.chunk_total), total: d.chunk_total };
          fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading image... (0/' + d.chunk_total + ')</p></div>';
        }
        const state = _imageChunks[d.path];
        state.chunks[d.chunk_index] = d.content;
        const received = state.chunks.filter(Boolean).length;
        if (received < state.total) {
          fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading image... (' + received + '/' + state.total + ')</p></div>';
          return;
        }
        // All chunks received — reassemble.
        const fullDataUri = state.chunks.join('');
        delete _imageChunks[d.path];
        fileContentBody.innerHTML = '<div class="file-image-view"><img src="' + fullDataUri + '" alt="' + escapeHtml(d.path) + '"></div>';
        return;
      }
      fileContentBody.innerHTML = '<div class="file-image-view"><img src="' + d.content + '" alt="' + escapeHtml(d.path) + '"></div>';
      return;
    }
    if (d.is_binary) {
      fileContentBody.innerHTML = '<div class="empty-state"><p>Binary file (' + formatBytes(d.size) + ')</p></div>';
      return;
    }
    renderFileContent(d.content, d.path, d.size, d.truncated);
  });

  instance.addEventListener('file_diff_result', (evt) => {
    const d = evt.detail;
    if (d.channel_id !== state.filesChannelId || d.path !== state.filesCurrentPath) return;
    if (state.filesCurrentView !== 'diff') return;

    if (!d.diff) {
      fileContentBody.innerHTML = '<div class="empty-state"><p>No changes</p></div>';
      return;
    }
    renderDiffContent(d.diff, d.truncated);
  });
} // end bindE2EEEvents
