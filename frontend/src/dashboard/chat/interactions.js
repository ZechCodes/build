import { state } from '../state.js';
import { escapeHtml } from '../util/html.js';
import { shortTime } from '../util/time.js';
import { agentShortName } from '../console/tools.js';
import { modelToFriendlyName } from '../util/format.js';
import { renderMarkdown } from '../vendor/markdown.js';
import { getActiveE2EE } from '../e2ee/bridge.js';
import { renderChannelList } from '../channels/list.js';

export function appendInteractionCard(msg, meta) {
  const container = document.getElementById('chat-messages');
  const empty = container.querySelector('.empty-state');
  if (empty) empty.remove();

  const resolved = !!meta.resolved_at;
  const interactionId = meta.interaction_id;
  const kind = meta.kind || 'question';
  const options = meta.options || [];
  const allowFreeform = meta.allow_freeform !== false;
  const multiselect = !!meta.multiselect;
  const plan = meta.plan || null;
  const timeStr = msg.created_at ? shortTime(
    typeof msg.created_at === 'number'
      ? new Date(msg.created_at * 1000).toISOString()
      : msg.created_at
  ) : '';

  // Plan review cards get a special light-themed layout.
  if (kind === 'plan_review') {
    appendPlanReviewCard(msg, meta, container, resolved, interactionId, options, plan, timeStr);
    return;
  }

  const div = document.createElement('div');
  div.className = `msg ${resolved ? 'interaction-resolved-msg' : ''}`;
  div.dataset.msgId = msg.id || '';
  if (msg.created_at) div.dataset.createdAt = typeof msg.created_at === 'number' ? new Date(msg.created_at * 1000).toISOString() : msg.created_at;

  const senderName = agentShortName(msg.sender);
  let cardHtml = `
    <div class="msg-avatar agent">${senderName[0]}</div>
    <div class="msg-body">
      <div class="msg-header">
        <span class="msg-name">${escapeHtml(senderName)}</span>
        <span class="msg-time">${timeStr}</span>
      </div>
      <div class="interaction-card ${resolved ? 'resolved' : ''}" data-interaction-id="${escapeHtml(interactionId)}">
        <div class="interaction-question">${renderMarkdown(msg.content || '')}</div>`;

  if (plan) {
    cardHtml += `
        <div class="interaction-plan-content">${renderMarkdown(plan)}</div>`;
  }

  if (resolved) {
    const selectedOpt = meta.selected_option || '';
    const selectedOpts = meta.selected_options || [];
    if (options.length) {
      cardHtml += `<div class="interaction-options">`;
      for (const opt of options) {
        const sel = (selectedOpts.length ? selectedOpts.includes(opt.id) : opt.id === selectedOpt) ? ' selected' : '';
        cardHtml += `<button class="interaction-opt${sel}" disabled>${escapeHtml(opt.label || opt.id)}</button>`;
      }
      cardHtml += `</div>`;
    }
    if (meta.freeform_response) {
      cardHtml += `<div class="interaction-freeform-response">${escapeHtml(meta.freeform_response)}</div>`;
    }
  } else if (multiselect) {
    if (options.length) {
      cardHtml += `<div class="interaction-options multiselect">`;
      for (const opt of options) {
        cardHtml += `<button class="interaction-opt" data-opt-id="${escapeHtml(opt.id)}">${escapeHtml(opt.label || opt.id)}</button>`;
      }
      cardHtml += `</div>`;
    }
    cardHtml += `
      <div class="interaction-freeform">
        ${allowFreeform ? '<textarea placeholder="Type a response..." rows="1"></textarea>' : ''}
        <button class="interaction-submit">Submit</button>
      </div>`;
  } else {
    if (options.length) {
      cardHtml += `<div class="interaction-options">`;
      for (const opt of options) {
        cardHtml += `<button class="interaction-opt" data-opt-id="${escapeHtml(opt.id)}">${escapeHtml(opt.label || opt.id)}</button>`;
      }
      cardHtml += `</div>`;
    }
    if (allowFreeform) {
      cardHtml += `
        <div class="interaction-freeform">
          <textarea placeholder="Type a response..." rows="1"></textarea>
          <button class="interaction-submit">Send</button>
        </div>`;
    }
  }

  cardHtml += `</div></div>`;
  div.innerHTML = cardHtml;
  container.appendChild(div);

  const card = div.querySelector('.interaction-card');
  if (!card || resolved) return;

  if (multiselect) {
    card.querySelectorAll('.interaction-opt').forEach(btn => {
      btn.addEventListener('click', () => {
        btn.classList.toggle('selected');
      });
    });
    const submitBtn = card.querySelector('.interaction-submit');
    if (submitBtn) {
      submitBtn.addEventListener('click', () => {
        const selected = [...card.querySelectorAll('.interaction-opt.selected')].map(b => b.dataset.optId);
        const textarea = card.querySelector('.interaction-freeform textarea');
        const freeform = textarea ? textarea.value.trim() : null;
        respondToMultiselectInteraction(interactionId, selected, freeform || null);
      });
    }
  } else {
    card.querySelectorAll('.interaction-opt').forEach(btn => {
      btn.addEventListener('click', () => {
        respondToInteraction(interactionId, btn.dataset.optId, null);
      });
    });
    const submitBtn = card.querySelector('.interaction-submit');
    if (submitBtn) {
      submitBtn.addEventListener('click', () => {
        respondToInteractionFreeform(submitBtn, interactionId);
      });
    }
  }
}

export function appendPlanReviewCard(msg, meta, container, resolved, interactionId, options, plan, timeStr) {
  const channelModel = state.chatChannels.get(state.chatCurrentChannel)?.model || '';
  const displayName = modelToFriendlyName(channelModel);

  const div = document.createElement('div');
  div.className = 'msg';
  div.dataset.msgId = msg.id || '';
  if (msg.created_at) div.dataset.createdAt = typeof msg.created_at === 'number' ? new Date(msg.created_at * 1000).toISOString() : msg.created_at;

  const selectedOption = meta.selected_option || '';
  const freeformResponse = meta.freeform_response || '';
  const wasApproved = selectedOption === 'approve';
  const wasDenied = selectedOption === 'reject';
  const gaveFeedback = !!freeformResponse;

  let actionsHtml = '';
  if (resolved) {
    const approveClass = `plan-review-btn approve${wasApproved ? ' selected' : ''}`;
    const denyClass = `plan-review-btn deny${wasDenied ? ' selected' : ''}`;
    actionsHtml = `
      <div class="plan-review-actions">
        <button class="${approveClass}" disabled>Approve</button>
        <button class="${denyClass}" disabled>Deny</button>
        ${gaveFeedback ? `<span class="plan-review-note">You provided further instructions</span>` : ''}
      </div>`;
  } else {
    actionsHtml = `
      <div class="plan-review-actions">
        <button class="plan-review-btn approve" data-opt-id="approve">Approve</button>
        <button class="plan-review-btn deny" data-opt-id="reject">Deny</button>
        <span class="plan-review-note">or send a message to provide feedback</span>
      </div>`;
  }

  const senderName = agentShortName(msg.sender) || displayName;
  div.innerHTML = `
    <div class="plan-review-card" data-interaction-id="${escapeHtml(interactionId)}">
      <div class="plan-review-title">
        <span class="plan-review-avatar">${senderName[0]}</span>
        ${escapeHtml(senderName)}'s Plan
        <span class="plan-review-time">${timeStr}</span>
      </div>
      <div class="plan-review-body">
        ${plan ? `<div class="plan-review-content">${renderMarkdown(plan)}</div>` : ''}
        ${actionsHtml}
      </div>
    </div>`;

  container.appendChild(div);

  if (resolved) return;

  const card = div.querySelector('.plan-review-card');
  card.querySelectorAll('.plan-review-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      const optId = btn.dataset.optId;
      respondToInteraction(interactionId, optId, null);
      card.querySelectorAll('.plan-review-btn').forEach(b => {
        b.disabled = true;
        if (b.dataset.optId === optId) b.classList.add('selected');
      });
      const note = card.querySelector('.plan-review-note');
      if (note) note.remove();
    });
  });
}

export function resolvePendingPlanReviews() {
  document.querySelectorAll('.plan-review-card').forEach(card => {
    const btns = card.querySelectorAll('.plan-review-btn:not(:disabled)');
    if (!btns.length) return;
    btns.forEach(b => { b.disabled = true; });
    const note = card.querySelector('.plan-review-note');
    if (note) note.textContent = 'You provided further instructions';
  });
  document.querySelectorAll('.interaction-card:not(.resolved)').forEach(card => {
    card.classList.add('resolved');
    const opts = card.querySelector('.interaction-options');
    const ff = card.querySelector('.interaction-freeform');
    if (opts) opts.remove();
    if (ff) ff.remove();
  });
}

export function respondToInteraction(interactionId, selectedOption, freeformResponse) {
  const _conn = getActiveE2EE();
  if (!_conn || !_conn.connected || !state.chatCurrentChannel) return;
  _conn.sendInteractionResponse(state.chatCurrentChannel, interactionId, selectedOption, freeformResponse);
  const card = document.querySelector(`[data-interaction-id="${interactionId}"]`);
  if (card) {
    card.classList.add('resolved');
    card.querySelectorAll('.interaction-opt').forEach(btn => {
      btn.disabled = true;
      if (btn.dataset.optId === selectedOption) btn.classList.add('selected');
    });
    const ff = card.querySelector('.interaction-freeform');
    if (ff) ff.remove();
  }
  const uc = state.unreadCounts.get(state.chatCurrentChannel);
  if (uc) {
    uc.hasInteraction = false;
    renderChannelList();
  }
}

export function crossfadeStatus(statusEl, newState, newContent) {
  statusEl.classList.remove('sending', 'delivered', 'read', 'failed');
  statusEl.classList.add(newState);
  const oldSpans = statusEl.querySelectorAll('.msg-status-state');
  const newSpan = document.createElement('span');
  newSpan.className = 'msg-status-state enter';
  newSpan.innerHTML = newContent;
  statusEl.appendChild(newSpan);
  oldSpans.forEach(oldSpan => {
    oldSpan.classList.remove('visible', 'enter');
    oldSpan.classList.add('exit');
    setTimeout(() => oldSpan.remove(), 300);
  });
  requestAnimationFrame(() => {
    newSpan.classList.remove('enter');
    newSpan.classList.add('visible');
  });
}

export function respondToInteractionFreeform(btn, interactionId) {
  const textarea = btn.parentElement.querySelector('textarea');
  const text = (textarea ? textarea.value : '').trim();
  if (!text) return;
  respondToInteraction(interactionId, null, text);
}

export function respondToMultiselectInteraction(interactionId, selectedOptions, freeformResponse) {
  const _conn = getActiveE2EE();
  if (!_conn || !_conn.connected || !state.chatCurrentChannel) return;
  _conn.sendInteractionResponse(state.chatCurrentChannel, interactionId, null, freeformResponse, selectedOptions);
  const card = document.querySelector(`[data-interaction-id="${interactionId}"]`);
  if (card) {
    card.classList.add('resolved');
    card.querySelectorAll('.interaction-opt').forEach(btn => {
      btn.disabled = true;
      if (!selectedOptions.includes(btn.dataset.optId)) btn.classList.remove('selected');
    });
    const ff = card.querySelector('.interaction-freeform');
    if (ff) ff.remove();
  }
  const uc = state.unreadCounts.get(state.chatCurrentChannel);
  if (uc) {
    uc.hasInteraction = false;
    renderChannelList();
  }
}

export function updatePlanModeUI(active) {
  const btn = document.getElementById('cmd-plan-btn');
  if (!btn) return;
  btn.classList.toggle('active', !!active);
}
