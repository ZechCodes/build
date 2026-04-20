import { state } from '../state.js';
import { escapeHtml, formatFileSize } from '../util/html.js';
import { shortTime } from '../util/time.js';
import { agentShortName } from '../console/tools.js';
import { renderMarkdown } from '../vendor/markdown.js';
import { markUnreadMessages } from '../channels/unread.js';
import { dismissStaleSuggestions } from '../channels/select.js';
import { createEmptyState, scrollToFirstUnread, scrollChatToBottom } from '../console/view.js';
import { appendInteractionCard } from './interactions.js';
import { sendChatMessage } from './composer.js';

export function renderMessages() {
  const container = document.getElementById('chat-messages');

  if (!state.chatCurrentChannel) {
    container.innerHTML = '';
    container.appendChild(createEmptyState('Select a channel', 'Choose or create a channel to start chatting.'));
    return;
  }

  const msgs = state.chatMessages.get(state.chatCurrentChannel) || [];

  if (msgs.length === 0) {
    container.innerHTML = '';
    const ch = state.chatChannels.get(state.chatCurrentChannel);
    if (state.chatLoadingMessages.has(state.chatCurrentChannel)) {
      container.appendChild(createEmptyState(
        `#${ch?.name || 'channel'}`,
        'Loading messages…',
      ));
    } else {
      container.appendChild(createEmptyState(
        `#${ch?.name || 'channel'}`,
        'No messages yet. Send one to get started.',
      ));
    }
    return;
  }

  container.innerHTML = '';
  for (const msg of msgs) {
    appendMessage(msg);
  }
  // Mark unread messages with warm background.
  markUnreadMessages(container);
  // Dismiss old suggestion buttons and restore selected state from history.
  dismissStaleSuggestions(container, msgs);
  scrollToFirstUnread(container) || scrollChatToBottom();
}

export function appendMessage(msg) {
  // Delegate to interaction card if metadata present.
  if (msg.metadata) {
    const meta = typeof msg.metadata === 'string' ? JSON.parse(msg.metadata) : msg.metadata;
    if (meta.interaction_id) {
      appendInteractionCard(msg, meta);
      return;
    }
  }

  const container = document.getElementById('chat-messages');
  const empty = container.querySelector('.empty-state');
  if (empty) empty.remove();

  const isUser = msg.sender === 'client';
  const avatarClass = isUser ? 'user' : 'agent';
  const displayName = isUser ? 'You' : agentShortName(msg.sender);
  const avatarLabel = isUser ? 'Y' : displayName[0];
  const nameLabel = displayName;
  const timeStr = msg.created_at ? shortTime(
    typeof msg.created_at === 'number'
      ? new Date(msg.created_at * 1000).toISOString()
      : msg.created_at
  ) : '';

  const div = document.createElement('div');
  div.className = 'msg';
  div.dataset.msgId = msg.id || '';
  if (msg.created_at) div.dataset.createdAt = typeof msg.created_at === 'number' ? new Date(msg.created_at * 1000).toISOString() : msg.created_at;
  div.innerHTML = `
    <div class="msg-avatar ${avatarClass}">${avatarLabel}</div>
    <div class="msg-body">
      <div class="msg-header">
        <span class="msg-name">${nameLabel}</span>
        <span class="msg-time">${timeStr}</span>
      </div>
      <div class="msg-text">${renderMarkdown(msg.content || '')}</div>
      ${msg.attachments && msg.attachments.length ? `<div class="msg-attachments">${
        msg.attachments.map(att =>
          `<div class="msg-attachment">
            <svg viewBox="0 0 16 16" fill="none"><path d="M14 8.5l-5.5 5.5a3.5 3.5 0 01-5-5L9 3.5a2.5 2.5 0 013.5 3.5L7 12.5a1.5 1.5 0 01-2-2L10.5 5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>
            <span class="att-name">${escapeHtml(att.filename || att.name || 'file')}</span>
            <span class="att-size">${formatFileSize(att.size || 0)}</span>
          </div>`
        ).join('')
      }</div>` : ''}
      ${isUser && msg.id ? `<div class="msg-status ${msg.read_at ? 'read' : msg.delivered_at ? 'delivered' : 'sending'}"><span class="msg-status-state visible"><span class="check ${msg.read_at || msg.delivered_at ? 'active' : ''}">✓</span><span class="check ${msg.read_at ? 'active' : ''}">✓</span> ${msg.read_at ? 'Read' : msg.delivered_at ? 'Delivered' : 'Sending'}</span></div>` : ''}
      ${msg.suggested_actions?.length ? `<div class="msg-suggestions">${
        msg.suggested_actions.map(a => `<button class="suggestion-btn">${escapeHtml(a)}</button>`).join('')
      }</div>` : ''}
    </div>
  `;
  // File/diff embed toggles.
  div.querySelectorAll('.build-embed-header').forEach(hdr => {
    hdr.style.cursor = 'pointer';
    hdr.addEventListener('click', (e) => {
      if (e.target.closest('.build-embed-wrap-toggle')) return;
      const embed = hdr.closest('.build-embed');
      embed.classList.toggle('collapsed');
    });
  });
  // Suggestion button clicks → send as user message.
  if (msg.suggested_actions?.length) {
    div.querySelectorAll('.suggestion-btn').forEach(btn => {
      btn.addEventListener('click', () => {
        if (btn.classList.contains('selected') || btn.classList.contains('dismissed')) return;
        div.querySelectorAll('.suggestion-btn').forEach(b => {
          if (b === btn) b.classList.add('selected');
          else b.classList.add('dismissed');
        });
        const input = document.getElementById('chat-input');
        if (input) { input.value = btn.textContent; sendChatMessage(); }
      });
    });
  }
  container.appendChild(div);
}

export function appendSystemMessage(text) {
  const container = document.getElementById('chat-messages');
  const div = document.createElement('div');
  div.className = 'system-msg';
  div.textContent = text;
  container.appendChild(div);
}

export function updateStopButton() {
  const btn = document.getElementById('chat-stop-btn');
  if (!btn) return;
  const active = state.chatCurrentChannel && state.channelAgentActive.get(state.chatCurrentChannel);
  btn.classList.toggle('visible', !!active);
}
