// ChatView — messages, composer, stop button, interaction cards.
// Mounts into #v2-tab-chat. One instance per Channel.
// See planning/dashboard-v2/05-views.md § ChatView.

import { bus } from '../../core/bus.js';
import { log } from '../../core/log.js';
import { messagesStore } from '../../domain/messages-store.js';
import { presenceStore } from '../../domain/presence-store.js';
import { channelsStore } from '../../domain/channels-store.js';
import { unreadStore } from '../../domain/unread-store.js';
import { escapeHtml, formatBytes } from '../../util/html.js';
import { shortTime } from '../../util/time.js';
import { renderMarkdown } from '../../util/markdown.js';
import { agentShortName, currentToolPhrase } from '../../util/tools.js';
import { currentToolStore } from '../../domain/current-tool-store.js';
import { uploadFile } from '../../transport/index.js';
import { showToast } from '../../util/toast.js';
import { e2eePool } from '../../transport/e2ee-pool.js';

const ilog = log('interaction');

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
    // On initial mount we always want the latest message in view.
    this._render({ forceBottom: true });
    // The overlay may only reach its final layout size after this tick
    // (it was just display-set-to-flex, fonts may still be loading, etc.)
    // Re-pin to bottom on the next frame and again after a short delay.
    requestAnimationFrame(() => this._scrollToBottom(true));
    setTimeout(() => this._scrollToBottom(true), 120);
    this.unsubs.push(messagesStore.subscribe(e => {
      if (e.channelId !== this.channel.id) return;
      // Single-message appends get special scroll handling so a long
      // message pins its TOP to the viewport top rather than its bottom.
      // A bulk (history) load forces bottom so the latest message is
      // visible on first paint.
      this._render({
        appendedOne: e.kind === 'append',
        forceBottom: e.kind === 'bulk',
      });
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
      if (e.kind === 'agent_active') this._syncToolStrip();
    }));
    this._toolShownEntry = null;   // entry | 'thinking' | null
    this._toolShownAt    = 0;      // ms when the current entry started showing
    this._toolIdleTimer  = null;   // deferred flip from tool → "Thinking"
    this.unsubs.push(currentToolStore.subscribe(e => {
      if (e.channelId !== this.channel.id) return;
      this._onCurrentToolChange(e.entry);
    }));
    this._syncToolStrip();
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
    // Clean up every listener we attached in _buildShell. Without
    // this, deactivate → activate (e.g. channel switching) leaks a
    // stale ChatView's click handler onto #v2-chat-overlay-body.
    // The leaked handler marks suggestion buttons as `.selected`
    // using stale `this`, which makes the new ChatView's handler
    // short-circuit on the "already selected" guard — so clicks
    // silently no-op until a page reload clears the DOM.
    if (this.root) {
      this.root.removeEventListener('click', this._onClick);
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
    if (this._initialScrollObs) { this._initialScrollObs.disconnect(); this._initialScrollObs = null; }
    this._closeModelPicker();
    this._dragDepth = 0;
    this.root = null;
    this.messagesEl = null;
    this.composerInput = null;
    this.toolbarEl = null;
    this.overlayEl = null;
    this.toolStripEl = null;
  }

  _buildShell() {
    // Body: messages + staging + composer input only. No buttons here;
    // action buttons live in the overlay toolbar (v1 parity).
    this.root.innerHTML = `
      <div class="v2-chat">
        <div class="v2-chat-messages-wrap">
          <div class="v2-chat-messages" data-slot="messages"></div>
          <div class="v2-chat-skeleton" aria-hidden="true">
            <div class="v2-skeleton v2-skeleton-bubble"></div>
            <div class="v2-skeleton v2-skeleton-bubble short"></div>
            <div class="v2-skeleton v2-skeleton-bubble"></div>
          </div>
          <button class="v2-chat-new-bubble" type="button" hidden>↓ New messages</button>
        </div>
        <div class="v2-chat-staging" data-slot="staging"></div>
        <div class="v2-chat-tool" data-slot="tool" hidden>
          <span class="v2-chat-tool-text"></span><span class="v2-chat-tool-ellipsis" aria-hidden="true"><span>.</span><span>.</span><span>.</span></span>
        </div>
        <div class="v2-chat-composer" data-slot="composer">
          <textarea class="v2-chat-input" rows="1" placeholder="Message…"></textarea>
        </div>
        <input type="file" class="v2-chat-file-input" multiple hidden>
      </div>
    `;
    this.messagesEl = this.root.querySelector('[data-slot="messages"]');
    this.composerInput = this.root.querySelector('.v2-chat-input');
    this.stagingEl = this.root.querySelector('[data-slot="staging"]');
    this.toolStripEl = this.root.querySelector('[data-slot="tool"]');
    this.fileInput = this.root.querySelector('.v2-chat-file-input');
    this.toolbarEl = document.getElementById('v2-chat-overlay-toolbar');
    this.overlayEl = document.getElementById('v2-chat-overlay');

    // Once on first paint: when the messages container becomes
    // taller than 0 (overlay settling into its flex height), pin to
    // bottom. Self-disconnects after the first non-zero dimension.
    if (typeof ResizeObserver !== 'undefined' && this.messagesEl) {
      this._initialScrollObs = new ResizeObserver((entries) => {
        for (const e of entries) {
          if (e.contentRect.height > 0) {
            this._scrollToBottom(true);
            this._initialScrollObs.disconnect();
            this._initialScrollObs = null;
            break;
          }
        }
      });
      this._initialScrollObs.observe(this.messagesEl);
    }

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

  _render({ appendedOne = false, forceBottom = false } = {}) {
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
    else if (forceBottom) this._scrollToBottom(true);
    else this._scrollToBottom(false);
  }

  _renderStopButton() {
    // Stop lives in the overlay toolbar, rebuilt by _renderToolbar.
    this._renderToolbar();
  }

  // ── Current-tool strip presenter ─────────────────────────────────────
  // Drives the strip above the composer. Rules:
  //   - A new tool_use always shows immediately (no queuing — newest
  //     wins). If the prior tool had been visible < 2s, we just let
  //     it go; the new one takes over.
  //   - When the current tool finishes (tool_result clears the store)
  //     we DON'T immediately flip to "Thinking". If the tool has been
  //     on screen < 2s, we wait out the remainder so the label doesn't
  //     flash and vanish. After the 2s mark (or immediately if we're
  //     already past it) we fall back to "Thinking" while the agent
  //     is still active, or hide if it's idle.
  //   - Agent goes idle → hide.

  _onCurrentToolChange(entry) {
    if (entry) {
      // New tool: replace whatever's showing, right now.
      this._clearIdleTimer();
      this._toolShownEntry = entry;
      this._toolShownAt    = Date.now();
      this._paintTool(currentToolPhrase(entry.name, entry.input || {}));
      return;
    }
    // Store cleared. If we were showing a tool that's been on screen
    // < 2s, defer the fallback so the label doesn't flash.
    if (this._toolShownEntry && this._toolShownEntry !== 'thinking') {
      const elapsed = Date.now() - this._toolShownAt;
      const remain  = Math.max(0, 2000 - elapsed);
      if (remain > 0) {
        this._clearIdleTimer();
        this._toolIdleTimer = setTimeout(() => {
          this._toolIdleTimer = null;
          this._syncToolStrip();
        }, remain);
        return;
      }
    }
    this._syncToolStrip();
  }

  _syncToolStrip() {
    const presence = presenceStore.get(this.channel.id);
    const rawEntry = currentToolStore.get(this.channel.id);

    if (rawEntry) {
      // A tool arrived while we had no strip mounted — paint it.
      if (this._toolShownEntry !== rawEntry) {
        this._clearIdleTimer();
        this._toolShownEntry = rawEntry;
        this._toolShownAt    = Date.now();
        this._paintTool(currentToolPhrase(rawEntry.name, rawEntry.input || {}));
      }
      return;
    }
    if (presence.agentActive) {
      if (this._toolShownEntry === 'thinking') return;
      this._clearIdleTimer();
      this._toolShownEntry = 'thinking';
      this._toolShownAt    = Date.now();
      this._paintTool('Thinking');
      return;
    }
    this._clearIdleTimer();
    this._toolShownEntry = null;
    this._paintTool(null);
  }

  _paintTool(text) {
    const strip = this.toolStripEl;
    if (!strip) return;
    if (text == null) {
      strip.hidden = true;
      return;
    }
    const textEl = strip.querySelector('.v2-chat-tool-text');
    if (textEl && textEl.textContent !== text) {
      textEl.textContent = text;
      textEl.classList.remove('swap');
      void textEl.offsetWidth;   // reflow so the fade animation restarts
      textEl.classList.add('swap');
    }
    strip.hidden = false;
  }

  _clearIdleTimer() {
    if (this._toolIdleTimer) {
      clearTimeout(this._toolIdleTimer);
      this._toolIdleTimer = null;
    }
  }

  _appendMessage(msg, allMsgs) {
    // Session dividers — client-side markers inserted by
    // transport/session-markers.js on reset / compact.
    if (msg.kind === 'session-divider') {
      const div = document.createElement('div');
      div.className = `v2-session-divider v2-session-divider-${escapeHtml(msg.divider_kind || 'event')}`;
      div.dataset.msgId = msg.id || '';
      div.innerHTML = `
        <span class="v2-session-divider-line" aria-hidden="true"></span>
        <span class="v2-session-divider-label">${escapeHtml(msg.content || '')}</span>
        <span class="v2-session-divider-line" aria-hidden="true"></span>
      `;
      this.messagesEl.appendChild(div);
      return;
    }
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
    const statusLabel = msg.delivery_failed ? 'Failed'
                      : msg.delivery_queued ? 'Queued'
                      : msg.read_at         ? 'Read'
                      : msg.delivered_at    ? 'Delivered'
                      : 'Sending';
    const statusCls   = msg.delivery_failed ? 'failed'
                      : msg.delivery_queued ? 'queued'
                      : msg.read_at         ? 'read'
                      : msg.delivered_at    ? 'delivered'
                      : 'sending';

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
    const questions = Array.isArray(meta.questions) ? meta.questions : null;
    const timeStr = msg.created_at
      ? shortTime(typeof msg.created_at === 'number' ? new Date(msg.created_at * 1000).toISOString() : msg.created_at)
      : '';

    // Multi-question paginated stepper. Hands off to a separate path
    // so the single-question code stays simple.
    if (!resolved && questions && questions.length > 1) {
      return this._appendStepperCard(msg, meta, questions, timeStr);
    }

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

  // ── Paginated multi-question stepper ─────────────────────────────
  // Rendered when the agent's AskUserQuestion sends >1 question. The
  // user walks through questions one at a time (click an option to
  // advance; Back to revisit). The last step's option click fires
  // `intent.interaction_response` with a `stepAnswers` array. Step
  // position + collected answers live on viewState keyed by
  // interaction id so they survive chat-view re-renders.

  _appendStepperCard(msg, meta, questions, timeStr) {
    const interactionId = meta.interaction_id;
    const senderName = agentShortName(msg.sender);

    const vs = this.channel.viewState;
    if (!vs.interactionSteps) vs.interactionSteps = {};
    const state = vs.interactionSteps[interactionId] || { step: 0, answers: [] };
    vs.interactionSteps[interactionId] = state;

    const div = document.createElement('div');
    div.dataset.msgId = msg.id || '';
    div.className = 'v2-msg agent';
    div.innerHTML = `
      <div class="v2-msg-avatar agent">${escapeHtml((senderName || 'A')[0])}</div>
      <div class="v2-msg-body">
        <div class="v2-msg-head">
          <span class="v2-msg-name">${escapeHtml(senderName)}</span>
          <span class="v2-msg-time">${escapeHtml(timeStr)}</span>
        </div>
        <div class="v2-int-card v2-int-stepper"
             data-interaction-id="${escapeHtml(interactionId)}"
             data-step="${state.step}"
             data-total="${questions.length}">
          <div class="v2-int-progress"></div>
          <div class="v2-int-step-body" data-slot="step-body"></div>
          <div class="v2-int-stepper-nav">
            <button class="v2-int-back" type="button" data-step-action="back">← Back</button>
            <span class="v2-int-step-label"></span>
          </div>
        </div>
      </div>
    `;
    this.messagesEl.appendChild(div);
    this._paintStep(div.querySelector('.v2-int-card'), questions, state);
  }

  _paintStep(cardEl, questions, state) {
    if (!cardEl) return;
    const total = questions.length;
    const step = Math.max(0, Math.min(state.step || 0, total - 1));
    const q = questions[step];
    const isLast = step === total - 1;

    cardEl.dataset.step = String(step);

    // Progress dots.
    const progress = cardEl.querySelector('.v2-int-progress');
    progress.innerHTML = questions.map((_, i) => {
      const cls = i === step ? 'active' : (i < step || state.answers[i] ? 'done' : 'pending');
      return `<span class="v2-int-dot ${cls}"></span>`;
    }).join('');

    // Step body: header + question text + options. Preserve the prior
    // answer for this step (if any) as `.selected` so the user sees
    // what they picked when they Back-navigate.
    const priorAnswer = state.answers[step];
    const bodyEl = cardEl.querySelector('[data-slot="step-body"]');
    const headerLine = q.header
      ? `<div class="v2-int-step-header">${escapeHtml(q.header)}</div>` : '';
    const questionLine = q.question
      ? `<div class="v2-int-question">${renderMarkdown(q.question)}</div>` : '';
    const opts = Array.isArray(q.options) ? q.options : [];
    const optsHtml = opts.length ? `
      <div class="v2-int-options">
        ${opts.map(o => {
          const sel = priorAnswer && priorAnswer.id === o.id;
          return `<button class="v2-int-opt${sel ? ' selected' : ''}" type="button"
                          data-step-opt-id="${escapeHtml(o.id)}"
                          data-step-opt-label="${escapeHtml(o.label || o.id)}">
                    ${escapeHtml(o.label || o.id)}
                  </button>`;
        }).join('')}
      </div>` : '';
    bodyEl.innerHTML = headerLine + questionLine + optsHtml;

    // Nav: Back enabled off step 0; label shows step N of M +
    // Submit hint on the last step.
    const backBtn = cardEl.querySelector('[data-step-action="back"]');
    backBtn.disabled = step === 0;
    const label = cardEl.querySelector('.v2-int-step-label');
    label.textContent = isLast
      ? `Step ${step + 1} of ${total} — choosing submits`
      : `Step ${step + 1} of ${total}`;
  }

  _onStepperOptionClick(cardEl, optId, optLabel) {
    const interactionId = cardEl.getAttribute('data-interaction-id');
    const questionsMeta = this._getStepperQuestions(interactionId);
    if (!questionsMeta) return;
    const vs = this.channel.viewState;
    const state = vs.interactionSteps[interactionId];
    if (!state) return;
    const total = questionsMeta.length;
    const step = Number(cardEl.dataset.step) || 0;
    state.answers[step] = { id: optId, label: optLabel };

    if (step === total - 1) {
      // Last step — collect all answers and submit.
      const stepAnswers = questionsMeta.map((q, i) => ({
        header:   q.header || '',
        question: q.question || '',
        answer:   state.answers[i]?.label || '',
        option_id: state.answers[i]?.id || '',
      }));
      bus.emit('intent.interaction_response', {
        channelId: this.channel.id,
        interactionId,
        selectedOption: null,
        freeformResponse: null,
        stepAnswers,
      });
      // Mark the card resolved locally so the user sees the stepper
      // replaced with a summary without waiting for a server echo.
      this._markStepperResolved(cardEl, questionsMeta, state.answers);
      // Clear per-interaction state — we're done.
      delete vs.interactionSteps[interactionId];
      return;
    }
    state.step = step + 1;
    this._paintStep(cardEl, questionsMeta, state);
  }

  _onStepperBack(cardEl) {
    const interactionId = cardEl.getAttribute('data-interaction-id');
    const questionsMeta = this._getStepperQuestions(interactionId);
    if (!questionsMeta) return;
    const state = this.channel.viewState.interactionSteps?.[interactionId];
    if (!state) return;
    if (state.step > 0) state.step -= 1;
    this._paintStep(cardEl, questionsMeta, state);
  }

  _getStepperQuestions(interactionId) {
    const msgs = messagesStore.forChannel(this.channel.id);
    const m = msgs.find(x => x.id === interactionId);
    if (!m || !m.metadata) return null;
    try {
      const meta = typeof m.metadata === 'string' ? JSON.parse(m.metadata) : m.metadata;
      return Array.isArray(meta.questions) && meta.questions.length > 1 ? meta.questions : null;
    } catch { return null; }
  }

  _markStepperResolved(cardEl, questions, answers) {
    const rows = questions.map((q, i) => {
      const a = answers[i]?.label || '—';
      const h = q.header || q.question || `Q${i + 1}`;
      return `<div class="v2-int-step-summary"><strong>${escapeHtml(h)}:</strong> ${escapeHtml(a)}</div>`;
    }).join('');
    cardEl.classList.add('resolved');
    cardEl.innerHTML = `
      <div class="v2-int-question">Your answers</div>
      ${rows}
    `;
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
    const interactionId = card?.getAttribute('data-interaction-id');
    if (!interactionId) {
      ilog.error('no interaction id on card', card);
      showToast('Could not submit — interaction id missing');
      return;
    }
    // Pre-flight: make sure there's a connected transport for this
    // channel. If the device is offline, the bus.emit would silently
    // no-op in the intent dispatcher and the optimistic UI would lie.
    const conn = e2eePool.forChannel(this.channel.id);
    if (!conn || !conn.connected) {
      ilog.warn('transport not connected; aborting interaction send', {
        interactionId, selectedOption, hasConn: !!conn,
      });
      showToast('Device disconnected — reconnect to respond');
      return;
    }
    ilog.info('sending interaction response', {
      interactionId, selectedOption, hasFreeform: !!freeformResponse,
      selectedOptions: selectedOptions?.length || 0,
    });
    bus.emit('intent.interaction_response', {
      channelId: this.channel.id,
      interactionId,
      selectedOption: selectedOption || null,
      freeformResponse: freeformResponse || null,
      selectedOptions: selectedOptions || null,
    });
    // Mirror the choice into the store so the next re-render shows
    // the chosen option as `.selected` + the card as `.resolved`.
    messagesStore.patchInteractionMeta(this.channel.id, interactionId, {
      resolved_at: new Date().toISOString(),
      selected_option: selectedOption || null,
      selected_options: selectedOptions || null,
      freeform_response: freeformResponse || null,
    });
    unreadStore.markRead(this.channel.id);
  }

  // ----- Composer / interaction click routing -----

  _onClick = (e) => {
    // Wrap toggle on build-embed / code block: flip .wrap-on, stop
    // further handling (don't also collapse).
    const wrapBtn = e.target.closest('.build-embed-wrap-toggle, .md-code-wrap-toggle');
    if (wrapBtn) {
      const block = wrapBtn.closest('.build-embed, .md-code-block');
      if (block) block.classList.toggle('wrap-on');
      e.stopPropagation();
      return;
    }
    // Copy button on code block.
    const copyBtn = e.target.closest('.md-code-copy');
    if (copyBtn) {
      const block = copyBtn.closest('.md-code-block');
      const code = block?.querySelector('pre code')?.textContent || '';
      if (code && navigator.clipboard) navigator.clipboard.writeText(code).catch(() => {});
      e.stopPropagation();
      return;
    }
    // Collapse-toggle when the user clicks a build-embed / code-block
    // header (anywhere other than the inline buttons handled above).
    const embedHeader = e.target.closest('.build-embed-header, .md-code-header');
    if (embedHeader) {
      const block = embedHeader.closest('.build-embed, .md-code-block');
      if (block) block.classList.toggle('collapsed');
      return;
    }
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
    // Model / effort picker toggle.
    if (e.target.closest('[data-cmd="change-model"]')) {
      this._toggleModelPicker();
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
    // Suggestion click → send as user message. Only `.selected` blocks
    // re-click; `.dismissed` (added when the user picks a sibling or
    // types a manual reply later) is visual-only — the user can still
    // click a dimmed suggestion if they change their mind.
    const sugBtn = e.target.closest('.v2-suggestion');
    if (sugBtn && !sugBtn.classList.contains('selected')) {
      const text = sugBtn.getAttribute('data-suggestion');
      sugBtn.parentElement.querySelectorAll('.v2-suggestion').forEach(b => {
        b.classList.remove('dismissed');
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
    // Stepper Back button — before the option handler so Back isn't
    // treated as an option click.
    const stepBack = e.target.closest('[data-step-action="back"]');
    if (stepBack && !stepBack.disabled) {
      const card = stepBack.closest('.v2-int-stepper');
      if (card) { this._onStepperBack(card); return; }
    }
    // Stepper option click — stored in step-opt-id so it doesn't
    // collide with the single-question option path.
    const stepOpt = e.target.closest('[data-step-opt-id]');
    if (stepOpt) {
      const card = stepOpt.closest('.v2-int-stepper');
      if (card) {
        this._onStepperOptionClick(
          card,
          stepOpt.getAttribute('data-step-opt-id'),
          stepOpt.getAttribute('data-step-opt-label'),
        );
        return;
      }
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
      <button class="v2-co-ctrl-pill" type="button" title="Change model or effort" data-cmd="change-model">
        <span class="v2-co-ctrl-part">${escapeHtml(model)}</span>
        ${effort ? `<span class="v2-co-ctrl-sep">·</span><span class="v2-co-ctrl-part">${escapeHtml(effort)}</span>` : ''}
      </button>
      <button class="v2-co-stop" type="button" data-action="stop" title="Stop agent" ${agentActive ? '' : 'hidden'}>
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><rect x="3" y="3" width="10" height="10" rx="1.5" fill="currentColor"/></svg>
      </button>
      <button class="v2-co-send" type="button" data-action="send" title="Send">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M2 8l12-5-5 12-2-5-5-2z" fill="currentColor"/></svg>
      </button>
    `;
  }

  _toggleModelPicker() {
    if (this._modelPicker) { this._closeModelPicker(); return; }
    this._openModelPicker();
  }

  _openModelPicker() {
    const ch = channelsStore.get(this.channel.id);
    if (!ch) return;
    const deviceId = channelsStore.deviceFor(this.channel.id);
    const harnesses = presenceStore.getHarnesses(deviceId) || [];
    const harness = harnesses.find(h => h.id === ch.harness) || harnesses[0] || null;
    const models = harness?.models || [];
    const effortLevels = harness?.effort_levels || ['low', 'medium', 'high'];
    const curModel = ch.model || '';
    const curEffort = ch.effort || '';

    const picker = document.createElement('div');
    picker.className = 'v2-model-picker';
    const modelsHtml = models.length
      ? models.map(m => `
          <button class="v2-model-picker-row ${m.id === curModel ? 'active' : ''}"
                  type="button" data-picker-model="${escapeHtml(m.id)}">
            <span class="v2-model-picker-label">${escapeHtml(m.name || m.id)}</span>
            ${m.provider ? `<span class="v2-model-picker-sub">${escapeHtml(m.provider)}</span>` : ''}
          </button>
        `).join('')
      : '<div class="v2-model-picker-empty">No models available</div>';
    const effortsHtml = effortLevels.length
      ? effortLevels.map(e => `
          <button class="v2-model-picker-row ${e === curEffort ? 'active' : ''}"
                  type="button" data-picker-effort="${escapeHtml(e)}">
            <span class="v2-model-picker-label">${escapeHtml(e)}</span>
          </button>
        `).join('')
      : '';
    picker.innerHTML = `
      <div class="v2-model-picker-section">
        <div class="v2-model-picker-header">Model</div>
        ${modelsHtml}
      </div>
      ${effortsHtml ? `
        <div class="v2-model-picker-section">
          <div class="v2-model-picker-header">Effort</div>
          ${effortsHtml}
        </div>
      ` : ''}
    `;
    document.body.appendChild(picker);
    this._modelPicker = picker;

    // Anchor above the chip.
    const trigger = this.toolbarEl?.querySelector('[data-cmd="change-model"]');
    if (trigger) {
      const r = trigger.getBoundingClientRect();
      const pickerRect = picker.getBoundingClientRect();
      const top = Math.max(8, r.top - pickerRect.height - 6);
      const right = Math.max(8, window.innerWidth - r.right);
      picker.style.position = 'fixed';
      picker.style.top = `${top}px`;
      picker.style.right = `${right}px`;
    }

    picker.addEventListener('click', (e) => {
      const mBtn = e.target.closest('[data-picker-model]');
      if (mBtn) {
        const modelId = mBtn.getAttribute('data-picker-model');
        if (modelId && modelId !== curModel) {
          bus.emit('intent.update_channel', {
            channelId: this.channel.id,
            patch: { model: modelId },
          });
        }
        this._closeModelPicker();
        return;
      }
      const eBtn = e.target.closest('[data-picker-effort]');
      if (eBtn) {
        const effort = eBtn.getAttribute('data-picker-effort');
        if (effort && effort !== curEffort) {
          bus.emit('intent.update_channel', {
            channelId: this.channel.id,
            patch: { effort },
          });
        }
        this._closeModelPicker();
      }
    });

    // Close on any click outside the picker or its trigger.
    this._modelPickerOutside = (e) => {
      if (e.target.closest('.v2-model-picker')) return;
      if (e.target.closest('[data-cmd="change-model"]')) return;
      this._closeModelPicker();
    };
    // Defer attach so the trigger's own click doesn't immediately close.
    setTimeout(() => document.addEventListener('click', this._modelPickerOutside), 0);
  }

  _closeModelPicker() {
    if (this._modelPicker) {
      this._modelPicker.remove();
      this._modelPicker = null;
    }
    if (this._modelPickerOutside) {
      document.removeEventListener('click', this._modelPickerOutside);
      this._modelPickerOutside = null;
    }
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
