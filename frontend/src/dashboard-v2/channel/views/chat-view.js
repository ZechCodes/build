// ChatView — messages, composer, stop button, interaction cards.
// Mounts into #v2-tab-chat. One instance per Channel.
// See planning/dashboard-v2/05-views.md § ChatView.

import { bus } from '../../core/bus.js';
import { messagesStore } from '../../domain/messages-store.js';
import { presenceStore } from '../../domain/presence-store.js';
import { channelsStore } from '../../domain/channels-store.js';
import { unreadStore } from '../../domain/unread-store.js';
import { escapeHtml, formatBytes } from '../../util/html.js';
import { shortTime } from '../../util/time.js';
import { renderMarkdown } from '../../util/markdown.js';
import { agentShortName } from '../../util/tools.js';
import { uploadFile } from '../../transport/index.js';
import { showToast } from '../../util/toast.js';

const CONFIRM_TIMEOUT_MS = 3000;

export class ChatView {
  constructor(channel) {
    this.channel = channel;
    this.root = null;
    this.messagesEl = null;
    this.composerInput = null;
    this.fileInput = null;
    this.stagingEl = null;
    this.unsubs = [];
    this._confirm = null;   // { kind, btn, timeout }
  }

  activate() {
    this.root = document.getElementById('v2-chat-overlay-body');
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
    this.unsubs.push(presenceStore.subscribe(e => {
      if (e.kind === 'plan_mode' && e.channelId === this.channel.id) this._renderCmdTray();
    }));
    this.unsubs.push(bus.on('upload.progress', (p) => {
      if (p.deviceId !== channelsStore.deviceFor(this.channel.id)) return;
      const chip = this.stagingEl?.querySelector(`[data-staging-name="${CSS.escape(p.fileName)}"]`);
      if (!chip) return;
      const bar = chip.querySelector('.v2-staging-bar-fill');
      if (bar) bar.style.width = `${Math.round((p.progress || 0) * 100)}%`;
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
        <div class="v2-chat-messages-wrap">
          <div class="v2-chat-messages" data-slot="messages"></div>
          <button class="v2-chat-new-bubble" type="button" hidden>↓ New messages</button>
        </div>
        <div class="v2-chat-staging" data-slot="staging"></div>
        <div class="v2-cmd-tray" data-slot="tray"></div>
        <div class="v2-chat-composer" data-slot="composer">
          <button class="v2-chat-stop" type="button" hidden data-action="stop">Stop</button>
          <textarea class="v2-chat-input" rows="1" placeholder="Message…"></textarea>
          <button class="v2-chat-send" type="button" data-action="send">Send</button>
        </div>
        <input type="file" class="v2-chat-file-input" multiple hidden>
      </div>
    `;
    this.messagesEl = this.root.querySelector('[data-slot="messages"]');
    this.composerInput = this.root.querySelector('.v2-chat-input');
    this.stagingEl = this.root.querySelector('[data-slot="staging"]');
    this.fileInput = this.root.querySelector('.v2-chat-file-input');
    this.composerInput.value = this.channel.viewState?.draftText || '';
    this._autosizeInput();
    this._renderCmdTray();
    this._renderStaging();

    this.root.addEventListener('click', this._onClick);
    this.composerInput.addEventListener('keydown', this._onKeydown);
    this.composerInput.addEventListener('input', this._autosizeInput);
    this.root.querySelector('.v2-chat-new-bubble').addEventListener('click', () => this._scrollToBottom(true));
    this.messagesEl.addEventListener('scroll', this._onScroll);
    this.fileInput.addEventListener('change', this._onFileSelect);

    // Drag-drop: files dropped on the message area get staged.
    this.messagesEl.addEventListener('dragover', (e) => { e.preventDefault(); this.messagesEl.classList.add('dragover'); });
    this.messagesEl.addEventListener('dragleave', () => this.messagesEl.classList.remove('dragover'));
    this.messagesEl.addEventListener('drop', this._onDrop);
  }

  _renderHeader() {
    // Overlay renders the channel name in its own header; nothing to do
    // here. Kept as a no-op for subscribe callbacks.
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
    // Attach — open file picker
    if (e.target.closest('[data-cmd="attach"]')) {
      this.fileInput?.click();
      return;
    }
    // Plan mode toggle — client-only; composer picks it up on send
    if (e.target.closest('[data-cmd="plan"]')) {
      const cur = presenceStore.get(this.channel.id).planMode;
      presenceStore.setPlanMode(this.channel.id, !cur);
      return;
    }
    // Compact / Reset — confirm pattern
    const confirmBtn = e.target.closest('[data-cmd-confirm]');
    if (confirmBtn) {
      this._handleConfirm(confirmBtn);
      return;
    }
    // Remove staged file
    const remove = e.target.closest('[data-staging-remove]');
    if (remove) {
      const name = remove.getAttribute('data-staging-remove');
      this._removePending(name);
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

  async _send() {
    const text = (this.composerInput?.value || '').trim();
    const pending = this.channel.viewState.pendingFiles || [];
    if (!text && !pending.length) return;

    // Disable composer while uploading.
    const sendBtn = this.root.querySelector('.v2-chat-send');
    if (sendBtn) sendBtn.disabled = true;
    if (this.composerInput) this.composerInput.disabled = true;

    let attachments = null;
    if (pending.length) {
      attachments = [];
      for (const file of pending) {
        try {
          const result = await uploadFile(this.channel.id, file);
          attachments.push({
            file_id: result.file_id,
            filename: result.filename,
            size: result.size,
            mime_type: result.mime_type,
          });
        } catch (err) {
          console.error('[ChatView] upload failed', err);
          showToast(`Upload failed: ${file.name}`);
        }
      }
      if (!attachments.length) attachments = null;
    }

    const planMode = presenceStore.get(this.channel.id).planMode;
    bus.emit('intent.send_message', {
      channelId: this.channel.id,
      text: text || (attachments ? `Sent ${attachments.length} file(s)` : ''),
      attachments,
      planMode,
    });

    // Reset composer.
    if (this.composerInput) {
      this.composerInput.value = '';
      this.composerInput.disabled = false;
      this._autosizeInput();
    }
    if (sendBtn) sendBtn.disabled = false;
    if (this.channel.viewState) {
      this.channel.viewState.draftText = '';
      this.channel.viewState.pendingFiles = [];
    }
    this._renderStaging();
    this.composerInput?.focus();
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

  // ----- Commands tray -----

  _renderCmdTray() {
    const tray = this.root?.querySelector('[data-slot="tray"]');
    if (!tray) return;
    const planOn = presenceStore.get(this.channel.id).planMode;
    tray.innerHTML = `
      <button class="v2-cmd-btn" type="button" data-cmd="attach" title="Attach files">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M14 8.5l-5.5 5.5a3.5 3.5 0 01-5-5L9 3.5a2.5 2.5 0 013.5 3.5L7 12.5a1.5 1.5 0 01-2-2L10.5 5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>
        <span>Attach</span>
      </button>
      <button class="v2-cmd-btn ${planOn ? 'active' : ''}" type="button" data-cmd="plan" title="Plan mode">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M3 3h10v3H3zM3 8h10v5H3z" stroke="currentColor" stroke-width="1.3"/></svg>
        <span>Plan</span>
      </button>
      <button class="v2-cmd-btn" type="button" data-cmd-confirm="compact" data-cmd="compact" title="Compact session">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M3 3h10M3 8h10M3 13h10" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>
        <span class="v2-cmd-label">Compact</span>
      </button>
      <button class="v2-cmd-btn" type="button" data-cmd-confirm="reset" data-cmd="reset" title="Reset session">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M13 8a5 5 0 10-1.8 3.8M13 4v4h-4" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>
        <span class="v2-cmd-label">Clear</span>
      </button>
    `;
  }

  _handleConfirm(btn) {
    const kind = btn.getAttribute('data-cmd-confirm');
    const label = btn.querySelector('.v2-cmd-label');
    // Second click within timeout → fire the intent.
    if (this._confirm?.btn === btn && btn.classList.contains('confirm')) {
      clearTimeout(this._confirm.timeout);
      btn.classList.remove('confirm');
      if (label) label.textContent = kind === 'compact' ? 'Compact' : 'Clear';
      this._confirm = null;
      if (kind === 'compact') bus.emit('intent.compact_session', { channelId: this.channel.id });
      else if (kind === 'reset') bus.emit('intent.reset_session', { channelId: this.channel.id });
      return;
    }
    // First click → arm.
    this._clearConfirm();
    btn.classList.add('confirm');
    if (label) label.textContent = 'Click to confirm';
    this._confirm = {
      kind, btn,
      timeout: setTimeout(() => {
        btn.classList.remove('confirm');
        if (label) label.textContent = kind === 'compact' ? 'Compact' : 'Clear';
        this._confirm = null;
      }, CONFIRM_TIMEOUT_MS),
    };
  }

  _clearConfirm() {
    if (!this._confirm) return;
    clearTimeout(this._confirm.timeout);
    const { btn, kind } = this._confirm;
    btn.classList.remove('confirm');
    const label = btn.querySelector('.v2-cmd-label');
    if (label) label.textContent = kind === 'compact' ? 'Compact' : 'Clear';
    this._confirm = null;
  }

  // ----- File staging -----

  _onFileSelect = (e) => {
    const files = [...(e.target.files || [])];
    if (!files.length) return;
    e.target.value = '';   // allow re-picking same file
    this._addPending(files);
  };

  _onDrop = (e) => {
    e.preventDefault();
    this.messagesEl?.classList.remove('dragover');
    const files = [...(e.dataTransfer?.files || [])];
    if (files.length) this._addPending(files);
  };

  _addPending(files) {
    const existing = this.channel.viewState.pendingFiles || [];
    // Dedup by name + size.
    const keys = new Set(existing.map(f => `${f.name}:${f.size}`));
    for (const f of files) {
      const k = `${f.name}:${f.size}`;
      if (!keys.has(k)) existing.push(f);
    }
    this.channel.viewState.pendingFiles = existing;
    this._renderStaging();
  }

  _removePending(name) {
    this.channel.viewState.pendingFiles =
      (this.channel.viewState.pendingFiles || []).filter(f => f.name !== name);
    this._renderStaging();
  }

  _renderStaging() {
    if (!this.stagingEl) return;
    const files = this.channel.viewState.pendingFiles || [];
    if (!files.length) { this.stagingEl.innerHTML = ''; this.stagingEl.hidden = true; return; }
    this.stagingEl.hidden = false;
    this.stagingEl.innerHTML = files.map(f => `
      <div class="v2-staging-chip" data-staging-name="${escapeHtml(f.name)}">
        <span class="v2-staging-name">${escapeHtml(f.name)}</span>
        <span class="v2-staging-size">${escapeHtml(formatBytes(f.size || 0))}</span>
        <div class="v2-staging-bar"><div class="v2-staging-bar-fill"></div></div>
        <button type="button" class="v2-staging-remove" data-staging-remove="${escapeHtml(f.name)}" aria-label="Remove">×</button>
      </div>
    `).join('');
  }
}
