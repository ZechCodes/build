import { state } from '../../state.js';
import { promoteChannel, renderChannelList, incrementUnread } from '../../channels/list.js';
import { setLastSeen, getLastSeen, deferMarkRead } from '../../channels/unread.js';
import { renderMessages, appendMessage, appendSystemMessage } from '../../chat/messages.js';
import { crossfadeStatus } from '../../chat/interactions.js';
import { isChatNearBottom, handleNewMessageScroll, scrollChatToBottom } from '../../console/view.js';

export function bindMessageHandlers(instance, deviceId) {
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

}
