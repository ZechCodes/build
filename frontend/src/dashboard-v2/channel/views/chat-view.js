// ChatView — messages, composer, stop button, interaction cards.
// Mounts into #v2-tab-chat. One instance per Channel.
// See planning/dashboard-v2/05-views.md § ChatView.

import { bus } from '../../core/bus.js';
import { messagesStore } from '../../domain/messages-store.js';
import { presenceStore } from '../../domain/presence-store.js';
import { channelsStore } from '../../domain/channels-store.js';
import { unreadStore } from '../../domain/unread-store.js';
import { escapeHtml } from '../../util/html.js';
import { shortTime } from '../../util/time.js';
import { renderMarkdown } from '../../util/markdown.js';
import { agentShortName } from '../../util/tools.js';

export class ChatView {
  constructor(channel) {
    this.channel = channel;
    this.root = null;
    this.messagesEl = null;
    this.composerInput = null;
    this.unsubs = [];
  }

  activate() {
    this.root = document.getElementById('v2-tab-chat');
    if (!this.root) return;
    this._buildShell();
    this._render();
    this.unsubs.push(messagesStore.subscribe(e => {
      if (e.channelId === this.channel.id) this._render();
    }));
    this.unsubs.push(presenceStore.subscribe(e => {
      if (e.channelId === this.channel.id) this._renderStopButton();
    }));
    this.unsubs.push(channelsStore.subscribe(e => {
      if (e.id === this.channel.id) this._renderHeader();
    }));
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this.composerInput && this.channel?.viewState) {
      this.channel.viewState.draftText = this.composerInput.value;
    }
    this.root = null;
    this.messagesEl = null;
    this.composerInput = null;
  }

  _buildShell() {
    this.root.innerHTML = `
      <div class="v2-chat">
        <header class="v2-chat-header" data-slot="header"></header>
        <div class="v2-chat-messages-wrap">
          <div class="v2-chat-messages" data-slot="messages"></div>
          <button class="v2-chat-new-bubble" type="button" hidden>↓ New messages</button>
        </div>
        <div class="v2-chat-composer" data-slot="composer">
          <button class="v2-chat-stop" type="button" hidden data-action="stop">Stop</button>
          <textarea class="v2-chat-input" rows="1" placeholder="Message…"></textarea>
          <button class="v2-chat-send" type="button" data-action="send">Send</button>
        </div>
      </div>
    `;
    this.messagesEl = this.root.querySelector('[data-slot="messages"]');
    this.composerInput = this.root.querySelector('.v2-chat-input');
    this.composerInput.value = this.channel.viewState?.draftText || '';
    this._autosizeInput();

    // Composer events (delegated on root for the two action buttons).
    this.root.addEventListener('click', this._onClick);
    this.composerInput.addEventListener('keydown', this._onKeydown);
    this.composerInput.addEventListener('input', this._autosizeInput);
    this.root.querySelector('.v2-chat-new-bubble').addEventListener('click', () => this._scrollToBottom(true));
    this.messagesEl.addEventListener('scroll', this._onScroll);
  }

  _renderHeader() {
    const el = this.root?.querySelector('[data-slot="header"]');
    if (!el) return;
    const ch = channelsStore.get(this.channel.id);
    const name = ch?.name || this.channel.id.slice(0, 8);
    el.innerHTML = `
      <span class="v2-chat-name">#${escapeHtml(name)}</span>
    `;
  }

  _render() {
    if (!this.messagesEl) return;
    this._renderHeader();
    this._renderStopButton();

    const msgs = messagesStore.forChannel(this.channel.id);
    if (msgs.length === 0) {
      this.messagesEl.innerHTML = '<div class="v2-chat-empty">No messages yet. Send one to get started.</div>';
      return;
    }

    this.messagesEl.innerHTML = '';
    msgs.forEach(m => this._appendMessage(m, msgs));
    this._dismissStaleSuggestions(msgs);
    this._scrollToBottom(false);
  }

  _renderStopButton() {
    const btn = this.root?.querySelector('.v2-chat-stop');
    if (!btn) return;
    const active = presenceStore.get(this.channel.id).agentActive;
    btn.hidden = !active;
  }

  _appendMessage(msg, allMsgs) {
    // Interaction cards use a dedicated renderer.
    if (msg.metadata) {
      let meta = null;
      try { meta = typeof msg.metadata === 'string' ? JSON.parse(msg.metadata) : msg.metadata; } catch (_) { /* ignore */ }
      if (meta?.interaction_id) {
        this._appendInteractionCard(msg, meta);
        return;
      }
    }

    const isUser = msg.sender === 'client';
    const displayName = isUser ? 'You' : agentShortName(msg.sender);
    const avatarLabel = isUser ? 'Y' : (displayName[0] || 'A');
    const timeStr = msg.created_at
      ? shortTime(typeof msg.created_at === 'number' ? new Date(msg.created_at * 1000).toISOString() : msg.created_at)
      : '';
    const statusLabel = msg.read_at ? 'Read' : msg.delivered_at ? 'Delivered' : 'Sending';
    const statusCls = msg.read_at ? 'read' : msg.delivered_at ? 'delivered' : 'sending';

    const div = document.createElement('div');
    div.className = 'v2-msg' + (isUser ? ' user' : ' agent');
    if (msg.id) div.dataset.msgId = msg.id;

    const suggestedHtml = Array.isArray(msg.suggested_actions) && msg.suggested_actions.length
      ? `<div class="v2-msg-suggestions">${msg.suggested_actions.map(a => `<button class="v2-suggestion" type="button" data-suggestion="${escapeHtml(a)}">${escapeHtml(a)}</button>`).join('')}</div>`
      : '';

    div.innerHTML = `
      <div class="v2-msg-avatar ${isUser ? 'user' : 'agent'}">${escapeHtml(avatarLabel)}</div>
      <div class="v2-msg-body">
        <div class="v2-msg-head">
          <span class="v2-msg-name">${escapeHtml(displayName)}</span>
          <span class="v2-msg-time">${escapeHtml(timeStr)}</span>
        </div>
        <div class="v2-msg-text">${renderMarkdown(msg.content || '')}</div>
        ${isUser && msg.id ? `<div class="v2-msg-status ${statusCls}">${escapeHtml(statusLabel)}</div>` : ''}
        ${suggestedHtml}
      </div>
    `;
    this.messagesEl.appendChild(div);
  }

  _appendInteractionCard(msg, meta) {
    const resolved = !!meta.resolved_at;
    const kind = meta.kind || 'question';
    const interactionId = meta.interaction_id;
    const options = meta.options || [];
    const allowFreeform = meta.allow_freeform !== false;
    const multiselect = !!meta.multiselect;
    const plan = meta.plan || null;
    const timeStr = msg.created_at
      ? shortTime(typeof msg.created_at === 'number' ? new Date(msg.created_at * 1000).toISOString() : msg.created_at)
      : '';

    const div = document.createElement('div');
    div.dataset.msgId = msg.id || '';

    if (kind === 'plan_review') {
      const senderName = agentShortName(msg.sender);
      div.className = 'v2-msg agent';
      const selectedOption = meta.selected_option || '';
      const wasApproved = selectedOption === 'approve';
      const wasDenied = selectedOption === 'reject';
      const gaveFeedback = !!meta.freeform_response;
      const actionsHtml = resolved
        ? `
          <div class="v2-plan-actions">
            <button class="v2-plan-btn approve${wasApproved ? ' selected' : ''}" disabled>Approve</button>
            <button class="v2-plan-btn deny${wasDenied ? ' selected' : ''}" disabled>Deny</button>
            ${gaveFeedback ? '<span class="v2-plan-note">You provided further instructions</span>' : ''}
          </div>`
        : `
          <div class="v2-plan-actions">
            <button class="v2-plan-btn approve" data-opt-id="approve">Approve</button>
            <button class="v2-plan-btn deny"    data-opt-id="reject">Deny</button>
            <span class="v2-plan-note">or send a message to provide feedback</span>
          </div>`;
      div.innerHTML = `
        <div class="v2-msg-avatar agent">${escapeHtml((senderName || 'A')[0])}</div>
        <div class="v2-msg-body">
          <div class="v2-plan-card" data-interaction-id="${escapeHtml(interactionId)}">
            <div class="v2-plan-title">${escapeHtml(senderName)}'s plan <span class="v2-plan-time">${escapeHtml(timeStr)}</span></div>
            ${plan ? `<div class="v2-plan-content">${renderMarkdown(plan)}</div>` : ''}
            ${actionsHtml}
          </div>
        </div>
      `;
      this.messagesEl.appendChild(div);
      return;
    }

    const senderName = agentShortName(msg.sender);
    div.className = 'v2-msg agent';
    let optsHtml = '';
    if (options.length) {
      optsHtml = `<div class="v2-int-options ${multiselect ? 'multiselect' : ''}">${
        options.map(o => {
          const sel = multiselect && meta.selected_options?.includes(o.id)
            || (!multiselect && (meta.selected_option === o.id));
          return `<button class="v2-int-opt${sel ? ' selected' : ''}${resolved ? ' disabled' : ''}" ${resolved ? 'disabled' : ''} data-opt-id="${escapeHtml(o.id)}">${escapeHtml(o.label || o.id)}</button>`;
        }).join('')
      }</div>`;
    }
    const resolvedFreeform = resolved && meta.freeform_response
      ? `<div class="v2-int-freeform-response">${escapeHtml(meta.freeform_response)}</div>`
      : '';
    const freeformHtml = (!resolved && allowFreeform)
      ? `<div class="v2-int-freeform">
           <textarea placeholder="Type a response…" rows="1"></textarea>
           <button class="v2-int-submit" type="button">${multiselect ? 'Submit' : 'Send'}</button>
         </div>`
      : (multiselect && !resolved
          ? `<div class="v2-int-freeform"><button class="v2-int-submit" type="button">Submit</button></div>`
          : '');

    div.innerHTML = `
      <div class="v2-msg-avatar agent">${escapeHtml((senderName || 'A')[0])}</div>
      <div class="v2-msg-body">
        <div class="v2-msg-head">
          <span class="v2-msg-name">${escapeHtml(senderName)}</span>
          <span class="v2-msg-time">${escapeHtml(timeStr)}</span>
        </div>
        <div class="v2-int-card${resolved ? ' resolved' : ''}" data-interaction-id="${escapeHtml(interactionId)}" data-multiselect="${multiselect ? '1' : '0'}">
          <div class="v2-int-question">${renderMarkdown(msg.content || '')}</div>
          ${plan ? `<div class="v2-int-plan-content">${renderMarkdown(plan)}</div>` : ''}
          ${optsHtml}
          ${resolvedFreeform}
          ${freeformHtml}
        </div>
      </div>
    `;
    this.messagesEl.appendChild(div);
  }

  _dismissStaleSuggestions(msgs) {
    // For each msg with suggested_actions, if a later client msg follows,
    // mark the selected one as .selected and the rest as .dismissed.
    const groups = this.messagesEl.querySelectorAll('.v2-msg-suggestions');
    if (!groups.length) return;
    const withSuggestions = [];
    for (let i = 0; i < msgs.length; i++) {
      if (!msgs[i].suggested_actions?.length) continue;
      let nextUserContent = null;
      for (let j = i + 1; j < msgs.length; j++) {
        if (msgs[j].sender === 'client') { nextUserContent = msgs[j].content; break; }
      }
      withSuggestions.push({ nextUserContent });
    }
    groups.forEach((group, idx) => {
      const info = withSuggestions[idx];
      if (!info || info.nextUserContent == null) return;
      group.querySelectorAll('.v2-suggestion').forEach(btn => {
        if (btn.textContent === info.nextUserContent) btn.classList.add('selected');
        else btn.classList.add('dismissed');
      });
    });
  }

  // ----- Interaction helpers -----

  _sendInteraction(card, selectedOption, freeformResponse, selectedOptions) {
    const interactionId = card.getAttribute('data-interaction-id');
    if (!interactionId) return;
    bus.emit('intent.interaction_response', {
      channelId: this.channel.id,
      interactionId,
      selectedOption: selectedOption || null,
      freeformResponse: freeformResponse || null,
      selectedOptions: selectedOptions || null,
    });
    card.classList.add('resolved');
    card.querySelectorAll('button').forEach(b => { b.disabled = true; });
    unreadStore.markRead(this.channel.id);
  }

  // ----- Composer / interaction click routing -----

  _onClick = (e) => {
    // Stop agent
    if (e.target.closest('[data-action="stop"]')) {
      bus.emit('intent.stop_agent', { channelId: this.channel.id });
      return;
    }
    // Send
    if (e.target.closest('[data-action="send"]')) {
      this._send();
      return;
    }
    // Suggestion click → send as user message
    const sugBtn = e.target.closest('.v2-suggestion');
    if (sugBtn && !sugBtn.classList.contains('selected') && !sugBtn.classList.contains('dismissed')) {
      const text = sugBtn.getAttribute('data-suggestion');
      sugBtn.parentElement.querySelectorAll('.v2-suggestion').forEach(b => {
        b.classList.add(b === sugBtn ? 'selected' : 'dismissed');
      });
      if (this.composerInput) this.composerInput.value = text;
      this._send();
      return;
    }
    // Plan-review buttons
    const planBtn = e.target.closest('.v2-plan-btn');
    if (planBtn && !planBtn.disabled) {
      const card = planBtn.closest('.v2-plan-card');
      this._sendInteraction(card, planBtn.getAttribute('data-opt-id'), null, null);
      return;
    }
    // Multi-select interaction toggle
    const intOpt = e.target.closest('.v2-int-opt');
    if (intOpt && !intOpt.disabled) {
      const card = intOpt.closest('.v2-int-card');
      if (card?.getAttribute('data-multiselect') === '1') {
        intOpt.classList.toggle('selected');
      } else {
        this._sendInteraction(card, intOpt.getAttribute('data-opt-id'), null, null);
      }
      return;
    }
    const intSubmit = e.target.closest('.v2-int-submit');
    if (intSubmit) {
      const card = intSubmit.closest('.v2-int-card');
      const multi = card?.getAttribute('data-multiselect') === '1';
      const textarea = card?.querySelector('textarea');
      const freeform = textarea ? textarea.value.trim() : null;
      if (multi) {
        const selected = [...card.querySelectorAll('.v2-int-opt.selected')].map(b => b.getAttribute('data-opt-id'));
        this._sendInteraction(card, null, freeform || null, selected);
      } else {
        if (!freeform) return;
        this._sendInteraction(card, null, freeform, null);
      }
      return;
    }
  };

  _onKeydown = (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      this._send();
    }
  };

  _send() {
    const text = (this.composerInput?.value || '').trim();
    if (!text) return;
    bus.emit('intent.send_message', { channelId: this.channel.id, text });
    if (this.composerInput) {
      this.composerInput.value = '';
      this._autosizeInput();
      if (this.channel.viewState) this.channel.viewState.draftText = '';
    }
  }

  _autosizeInput = () => {
    if (!this.composerInput) return;
    this.composerInput.style.height = 'auto';
    this.composerInput.style.height = Math.min(240, this.composerInput.scrollHeight) + 'px';
  };

  _onScroll = () => {
    if (!this.messagesEl) return;
    const nearBottom = this.messagesEl.scrollHeight - (this.messagesEl.scrollTop + this.messagesEl.clientHeight) < 40;
    const bubble = this.root.querySelector('.v2-chat-new-bubble');
    if (bubble && nearBottom) bubble.hidden = true;
  };

  _scrollToBottom(force) {
    if (!this.messagesEl) return;
    const nearBottom = this.messagesEl.scrollHeight - (this.messagesEl.scrollTop + this.messagesEl.clientHeight) < 80;
    if (force || nearBottom) {
      this.messagesEl.scrollTop = this.messagesEl.scrollHeight;
      const bubble = this.root.querySelector('.v2-chat-new-bubble');
      if (bubble) bubble.hidden = true;
    } else {
      const bubble = this.root.querySelector('.v2-chat-new-bubble');
      if (bubble) bubble.hidden = false;
    }
  }
}
