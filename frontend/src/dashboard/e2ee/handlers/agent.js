import { state } from '../../state.js';
import { promoteChannel, renderChannelList, incrementUnread } from '../../channels/list.js';
import { setLastSeen, deferMarkRead } from '../../channels/unread.js';
import { appendMessage, updateStopButton } from '../../chat/messages.js';
import { appendInteractionCard, updatePlanModeUI } from '../../chat/interactions.js';
import { agentShortName, describeToolUse, formatToolDetail, formatToolResult } from '../../console/tools.js';
import {
  isChatNearBottom,
  isConsoleNearBottom,
  handleNewMessageScroll,
  appendConsoleReasoning,
  appendConsoleEntry,
  markConsoleEntryDone,
  clearConsole,
  endReasoningEntry,
} from '../../console/view.js';
import { renderTasksPanel, updateTasksBadge } from '../../tasks/panel.js';
import { setConsoleState } from '../../shell/rail.js';
import { crossfadeStatus } from '../../chat/interactions.js';
import { onAgentFileChanges } from '../../files/tree.js';

export function bindAgentHandlers(instance, deviceId) {
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
  instance.addEventListener('e2ee_error', (evt) => {
    console.error('[E2EE] Error:', evt.detail);
  });
  instance.addEventListener('plan_mode_updated', (evt) => {
    const { channel_id, plan_mode } = evt.detail;
    state.channelPlanMode.set(channel_id, plan_mode);
    if (channel_id === state.chatCurrentChannel) updatePlanModeUI(plan_mode);
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
      endReasoningEntry();
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
          endReasoningEntry();
          appendConsoleReasoning(content, entry.created_at);
          endReasoningEntry();
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
}
