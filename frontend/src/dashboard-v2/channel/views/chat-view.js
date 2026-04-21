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
      if (e.channelId !== this.channel.id) return;
      // Single-message appends get special scroll handling so a long
      // message pins its TOP to the viewport top rather than its bottom.
      this._render({ appendedOne: e.kind === 'append' });
    }));
    this.unsubs.push(presenceStore.subscribe(e => {
      if (e.channelId === this.channel.id) this._renderStopButton();
    }));
    this.unsubs.push(channelsStore.subscribe(e => {
      if (e.id === this.channel.id) this._renderHeader();
    }));
    this.unsubs.push(presenceStore.subscribe(e => {
      if (e.channelId !== this.channel.id) return;
      if (e.kind === 'plan_mode' || e.kind === 'agent_active') this._renderToolbar();
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
    if (this.toolbarEl) {
      this.toolbarEl.removeEventListener('click', this._onClick);
      this.toolbarEl.innerHTML = '';
    }
    if (this.overlayEl) {
      this.overlayEl.removeEventListener('dragenter', this._onDragEnter);
      this.overlayEl.removeEventListener('dragover', this._onDragOver);
      this.overlayEl.removeEventListener('dragleave', this._onDragLeave);
      this.overlayEl.removeEventListener('drop', this._onDrop);
      this.overlayEl.classList.remove('v2-chat-drag-active');
    }
    if (this._dropzoneEl) { this._dropzoneEl.remove(); this._dropzoneEl = null; }
    this._dragDepth = 0;
    this.root = null;
    this.messagesEl = null;
    this.composerInput = null;
    this.toolbarEl = null;
    this.overlayEl = null;
  }

  _buildShell() {
    // Body: messages + staging + composer input only. No buttons here;
    // action buttons live in the overlay toolbar (v1 parity).
    this.root.innerHTML = `
      <div class="v2-chat">
        <div class="v2-chat-messages-wrap">
          <div class="v2-chat-messages" data-slot="messages"></div>
          <button class="v2-chat-new-bubble" type="button" hidden>↓ New messages</button>
        </div>
        <div class="v2-chat-staging" data-slot="staging"></div>
        <div class="v2-chat-composer" data-slot="composer">
          <textarea class="v2-chat-input" rows="1" placeholder="Message…"></textarea>
        </div>
        <input type="file" class="v2-chat-file-input" multiple hidden>
      </div>
    `;
    this.messagesEl = this.root.querySelector('[data-slot="messages"]');
    this.composerInput = this.root.querySelector('.v2-chat-input');
    this.stagingEl = this.root.querySelector('[data-slot="staging"]');
    this.fileInput = this.root.querySelector('.v2-chat-file-input');
    this.toolbarEl = document.getElementById('v2-chat-overlay-toolbar');
    this.overlayEl = document.getElementById('v2-chat-overlay');

    this.composerInput.value = this.channel.viewState?.draftText || '';
    this._autosizeInput();
    this._renderToolbar();
    this._renderStaging();

    // Add a drop-zone overlay inside the overlay container.
    if (this.overlayEl && !this.overlayEl.querySelector('.v2-chat-dropzone')) {
      const dz = document.createElement('div');
      dz.className = 'v2-chat-dropzone';
      dz.innerHTML = '<span>Drop files to attach</span>';
      this.overlayEl.appendChild(dz);
      this._dropzoneEl = dz;
    }

    this.root.addEventListener('click', this._onClick);
    if (this.toolbarEl) this.toolbarEl.addEventListener('click', this._onClick);
    this.composerInput.addEventListener('keydown', this._onKeydown);
    this.composerInput.addEventListener('input', this._autosizeInput);
    this.composerInput.addEventListener('paste', this._onPaste);
    this.root.querySelector('.v2-chat-new-bubble').addEventListener('click', () => this._scrollToBottom(true));
    this.messagesEl.addEventListener('scroll', this._onScroll);
    this.fileInput.addEventListener('change', this._onFileSelect);

    // Drag-and-drop covers the entire overlay so users can drop anywhere.
    if (this.overlayEl) {
      this.overlayEl.addEventListener('dragenter', this._onDragEnter);
      this.overlayEl.addEventListener('dragover', this._onDragOver);
      this.overlayEl.addEventListener('dragleave', this._onDragLeave);
      this.overlayEl.addEventListener('drop', this._onDrop);
    }
  }

  _renderHeader() {
    // Overlay renders the channel name in its own header; nothing to do
    // here. Kept as a no-op for subscribe callbacks.
  }

  _render({ appendedOne = false } = {}) {
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

    if (appendedOne) this._scrollNewIntoView();
    else this._scrollToBottom(false);
  }

  _renderStopButton() {
    // Stop lives in the overlay toolbar, rebuilt by _renderToolbar.
    this._renderToolbar();
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

  /**
   * Scroll handling for newly-appended messages.
   *   - If the new message fits in the viewport: scroll to bottom so
   *     the full message is visible.
   *   - If it's taller than the viewport: pin its TOP to the viewport
   *     top so the reader starts at the beginning.
   */
  _scrollNewIntoView() {
    if (!this.messagesEl) return;
    const last = this.messagesEl.lastElementChild;
    if (!last || !last.classList?.contains('v2-msg')) return;
    const viewportH = this.messagesEl.clientHeight;
    const msgH = last.getBoundingClientRect().height;
    if (msgH > viewportH) {
      // offsetTop is relative to the messages container (positioning
      // context). That puts the message's top at the container's top.
      this.messagesEl.scrollTop = last.offsetTop;
    } else {
      this.messagesEl.scrollTop = this.messagesEl.scrollHeight;
    }
    const bubble = this.root.querySelector('.v2-chat-new-bubble');
    if (bubble) bubble.hidden = true;
  }

  // ----- Overlay toolbar (v1 parity: Attach / Plan / Compact / Clear +
  //        spacer + model·effort pill + Stop + Send) -----

  _renderToolbar() {
    if (!this.toolbarEl) return;
    const presence = presenceStore.get(this.channel.id);
    const planOn = presence.planMode;
    const agentActive = presence.agentActive;
    const ch = channelsStore.get(this.channel.id);
    const model = ch?.model || '—';
    const effort = ch?.effort || '';
    this.toolbarEl.innerHTML = `
      <button class="v2-co-tool" type="button" data-cmd="attach" title="Attach file">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M14 8.5l-5.5 5.5a3.5 3.5 0 01-5-5L9 3.5a2.5 2.5 0 013.5 3.5L7 12.5a1.5 1.5 0 01-2-2L10.5 5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>
      </button>
      <button class="v2-co-tool ${planOn ? 'active' : ''}" type="button" data-cmd="plan" title="Plan mode">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M13 2H3a1 1 0 00-1 1v10a1 1 0 001 1h10a1 1 0 001-1V3a1 1 0 00-1-1z" stroke="currentColor" stroke-width="1.2"/><path d="M5 5h6M5 8h6M5 11h3" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
      </button>
      <button class="v2-co-tool" type="button" data-cmd-confirm="compact" data-cmd="compact" title="Compact conversation">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M4 2v4l4-4M12 2v4L8 2M4 14v-4l4 4M12 14v-4l-4 4" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>
        <span class="v2-cmd-label" hidden>Compact</span>
      </button>
      <button class="v2-co-tool" type="button" data-cmd-confirm="reset" data-cmd="reset" title="Clear conversation">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><circle cx="8" cy="8" r="6.5" stroke="currentColor" stroke-width="1.3"/><path d="M5.5 5.5l5 5M10.5 5.5l-5 5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>
        <span class="v2-cmd-label" hidden>Clear</span>
      </button>
      <div class="v2-co-tool-spacer"></div>
      <div class="v2-co-ctrl-pill" title="Model / effort">
        <span class="v2-co-ctrl-part">${escapeHtml(model)}</span>
        ${effort ? `<span class="v2-co-ctrl-sep">·</span><span class="v2-co-ctrl-part">${escapeHtml(effort)}</span>` : ''}
      </div>
      <button class="v2-co-stop" type="button" data-action="stop" title="Stop agent" ${agentActive ? '' : 'hidden'}>
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><rect x="3" y="3" width="10" height="10" rx="1.5" fill="currentColor"/></svg>
      </button>
      <button class="v2-co-send" type="button" data-action="send" title="Send">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M2 8l12-5-5 12-2-5-5-2z" fill="currentColor"/></svg>
      </button>
    `;
  }

  _handleConfirm(btn) {
    const kind = btn.getAttribute('data-cmd-confirm');
    // Second click within timeout → fire the intent.
    if (this._confirm?.btn === btn && btn.classList.contains('confirm')) {
      clearTimeout(this._confirm.timeout);
      btn.classList.remove('confirm');
      btn.title = kind === 'compact' ? 'Compact conversation' : 'Clear conversation';
      this._confirm = null;
      if (kind === 'compact') bus.emit('intent.compact_session', { channelId: this.channel.id });
      else if (kind === 'reset') bus.emit('intent.reset_session', { channelId: this.channel.id });
      return;
    }
    // First click → arm.
    this._clearConfirm();
    btn.classList.add('confirm');
    btn.title = 'Click again to confirm';
    this._confirm = {
      kind, btn,
      timeout: setTimeout(() => {
        btn.classList.remove('confirm');
        btn.title = kind === 'compact' ? 'Compact conversation' : 'Clear conversation';
        this._confirm = null;
      }, CONFIRM_TIMEOUT_MS),
    };
  }

  _clearConfirm() {
    if (!this._confirm) return;
    clearTimeout(this._confirm.timeout);
    const { btn, kind } = this._confirm;
    btn.classList.remove('confirm');
    btn.title = kind === 'compact' ? 'Compact conversation' : 'Clear conversation';
    this._confirm = null;
  }

  // ----- File staging -----

  _onFileSelect = (e) => {
    const files = [...(e.target.files || [])];
    if (!files.length) return;
    e.target.value = '';   // allow re-picking same file
    this._addPending(files);
  };

  _onDragEnter = (e) => {
    if (!e.dataTransfer || ![...(e.dataTransfer.types || [])].includes('Files')) return;
    e.preventDefault();
    this._dragDepth = (this._dragDepth || 0) + 1;
    this.overlayEl?.classList.add('v2-chat-drag-active');
  };

  _onDragOver = (e) => {
    if (!e.dataTransfer || ![...(e.dataTransfer.types || [])].includes('Files')) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
  };

  _onDragLeave = (e) => {
    // dragenter/leave fire per child element; track depth so flicker is
    // eliminated.
    this._dragDepth = Math.max(0, (this._dragDepth || 0) - 1);
    if (this._dragDepth === 0) this.overlayEl?.classList.remove('v2-chat-drag-active');
  };

  _onDrop = (e) => {
    if (!e.dataTransfer) return;
    e.preventDefault();
    this._dragDepth = 0;
    this.overlayEl?.classList.remove('v2-chat-drag-active');
    const files = [...(e.dataTransfer.files || [])];
    if (files.length) this._addPending(files);
  };

  _onPaste = (e) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    const files = [];
    for (const it of items) {
      if (it.kind === 'file') {
        const f = it.getAsFile();
        if (!f) continue;
        // Clipboard-pasted images usually have name === "image.png" or
        // similar. Keep the given name; if missing, synthesize one.
        if (!f.name) {
          const ext = (f.type.split('/')[1] || 'bin');
          try {
            const renamed = new File([f], `pasted-${Date.now()}.${ext}`, { type: f.type });
            files.push(renamed);
            continue;
          } catch (_) { /* fall through */ }
        }
        files.push(f);
      }
    }
    if (!files.length) return;
    // Only swallow the paste event if we actually grabbed file(s);
    // otherwise let text paste proceed normally.
    e.preventDefault();
    this._addPending(files);
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
