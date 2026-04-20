import { state } from '../state.js';
import { escapeHtml, escHtml } from '../util/html.js';
import { fmtClock24 } from '../util/time.js';
import { toolTag, agentShortName, describeToolUse, formatToolDetail, formatToolResult } from './tools.js';
import { renderMarkdown } from '../vendor/markdown.js';
import { toggleConsoleEntry } from '../shell/rail.js';
import { markUnreadMessages, getLastSeen } from '../channels/unread.js';

let currentReasoningEntry = null;

export function clearConsole(loading) {
  const body = document.querySelector('[data-console-panel="activity"]');
  if (body) {
    if (loading) {
      body.innerHTML = '<div class="empty-state"><p>Loading tool uses…</p></div>';
    } else {
      body.innerHTML = '';
    }
  }
  currentReasoningEntry = null;
}

export function appendConsoleReasoning(content, timestamp) {
  const body = document.querySelector('[data-console-panel="activity"]');
  if (!body) return;

  // Buffer into existing reasoning entry if one is active.
  if (currentReasoningEntry) {
    currentReasoningEntry._reasoningText += content;
    const desc = currentReasoningEntry.querySelector('.ce-desc');
    if (desc) {
      const firstLine = currentReasoningEntry._reasoningText.split('\n').find(l => l.trim()) || '';
      desc.textContent = firstLine;
    }
    const detail = currentReasoningEntry.querySelector('.ce-detail-reasoning');
    if (detail) detail.innerHTML = renderMarkdown(currentReasoningEntry._reasoningText);
    body.scrollTop = body.scrollHeight;
    return;
  }

  const ts = timestamp ? new Date(timestamp) : new Date();
  const timeStr = ts.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  const firstLine = content.split('\n').find(l => l.trim()) || content.slice(0, 100);

  const entry = document.createElement('div');
  entry.className = 'console-entry';
  entry._reasoningText = content;

  entry.innerHTML = `
    <div class="ce-row">
      <span class="ce-tag reasoning">Reasoning</span>
      <span class="ce-desc">${escapeHtml(firstLine)}</span>
      <span class="ce-time" data-ts="${ts.getTime()}"></span>
    </div>
    <div class="ce-detail">
      <div class="ce-detail-reasoning">${renderMarkdown(content)}</div>
    </div>
  `;

  const row = entry.querySelector('.ce-row');
  const detail = entry.querySelector('.ce-detail');
  row.addEventListener('click', () => {
    detail.classList.toggle('open');
    row.classList.toggle('open', detail.classList.contains('open'));
    if (detail.classList.contains('open')) {
      entry.scrollIntoView({ block: 'start', behavior: 'smooth' });
    }
  });

  const wasNearBottom = isConsoleNearBottom();
  body.appendChild(entry);
  renderConsoleTimes();
  if (wasNearBottom) body.scrollTop = body.scrollHeight;
  else showActivityBubble(entry);
  currentReasoningEntry = entry;
}

export function appendConsoleEntry(toolId, name, desc, input, timestamp) {
  const body = document.querySelector('[data-console-panel="activity"]');
  if (!body) return;

  const ts = timestamp ? new Date(timestamp) : new Date();
  const timeStr = ts.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  const tag = toolTag(name);

  const entry = document.createElement('div');
  entry.className = 'console-entry';
  if (toolId) entry.dataset.toolId = toolId;

  entry.innerHTML = `
    <div class="ce-row">
      <span class="ce-tag ${tag}">${escapeHtml(name)}</span>
      <span class="ce-desc">${escapeHtml(desc)}</span>
      <span class="ce-summary"></span>
      <span class="ce-time" data-ts="${ts.getTime()}"></span>
    </div>
    <div class="ce-detail">
      <div class="ce-detail-input">${formatToolDetail(name, input)}</div>
      <div class="ce-detail-result" id="ce-result-${toolId || ''}"></div>
    </div>
  `;
  entry._toolName = name;

  // Toggle detail on click.
  const row = entry.querySelector('.ce-row');
  const detail = entry.querySelector('.ce-detail');
  row.addEventListener('click', () => {
    detail.classList.toggle('open');
    row.classList.toggle('open', detail.classList.contains('open'));
    if (detail.classList.contains('open')) {
      entry.scrollIntoView({ block: 'start', behavior: 'smooth' });
    }
  });

  const wasNearBottom = isConsoleNearBottom();
  body.appendChild(entry);
  renderConsoleTimes();
  if (wasNearBottom) body.scrollTop = body.scrollHeight;
  else showActivityBubble(entry);
}

export function markConsoleEntryDone(toolId, isError, content, completedAt) {
  if (!toolId) return;
  const entry = document.querySelector(`.console-entry[data-tool-id="${toolId}"]`);
  if (!entry) return;
  const summary = entry.querySelector('.ce-summary');
  if (summary) {
    if (isError) {
      summary.innerHTML = '<span class="fail" title="error" aria-label="error">✗</span>';
    } else {
      summary.innerHTML = '<span class="success" title="done" aria-label="done">✓</span>';
    }
  }
  // Update displayed time to completion time.
  if (completedAt) {
    const timeEl = entry.querySelector('.ce-time');
    if (timeEl) {
      const ts = new Date(completedAt);
      timeEl.dataset.ts = String(ts.getTime());
      renderConsoleTimes();
    }
  }
  // Render result content in the expandable detail section.
  if (content) {
    const resultEl = entry.querySelector('.ce-detail-result');
    if (resultEl) {
      resultEl.innerHTML = formatToolResult(entry._toolName || '', content, isError);
    }
  }
}


export function isChatNearBottom(threshold = 80) {
  const c = document.getElementById('chat-messages');
  return c.scrollHeight - c.scrollTop - c.clientHeight < threshold;
}

export function isConsoleNearBottom(threshold = 80) {
  const c = document.querySelector('[data-console-panel="activity"]');
  if (!c) return true;
  return c.scrollHeight - c.scrollTop - c.clientHeight < threshold;
}

export function scrollChatToBottom() {
  const container = document.getElementById('chat-messages');
  container.scrollTop = container.scrollHeight;
  hideChatBubble();
}

// ---- Chat scroll engine ----

/**
 * Scroll so a message element is visible in the chat container.
 * align='auto': short messages → bottom-align (max context above), tall → top-align.
 * align='top': always top-align (for unread targets).
 */
export function scrollToMessage(el, behavior = 'instant', align = 'auto') {
  const container = document.getElementById('chat-messages');
  const cRect = container.getBoundingClientRect();
  const eRect = el.getBoundingClientRect();
  const elTop = eRect.top - cRect.top + container.scrollTop;
  const elH = eRect.height;
  const vpH = container.clientHeight;
  let target;
  if (align === 'top' || (align === 'auto' && elH >= vpH)) {
    target = elTop;
  } else {
    target = elTop + elH - vpH;
  }
  target = Math.max(0, Math.min(target, container.scrollHeight - vpH));
  if (behavior === 'smooth') {
    container.scrollTo({ top: target, behavior: 'smooth' });
  } else {
    container.scrollTop = target;
  }
}

let _oldestAutoScrollTarget = null;
let _scrollRAF = null;

/**
 * Called after appending a new incoming message.
 * wasNearBottom: result of isChatNearBottom() captured BEFORE the append.
 * newEl: the newly appended DOM element.
 */
export function handleNewMessageScroll(wasNearBottom, newEl) {
  if (!wasNearBottom) {
    showChatBubble(newEl);
    return;
  }
  // Track the oldest unread message that arrived while user was at bottom.
  if (!_oldestAutoScrollTarget) _oldestAutoScrollTarget = newEl;

  // Debounce: if multiple messages arrive in one frame, only scroll once.
  if (_scrollRAF) cancelAnimationFrame(_scrollRAF);
  _scrollRAF = requestAnimationFrame(() => {
    _scrollRAF = null;
    const container = document.getElementById('chat-messages');
    const cRect = container.getBoundingClientRect();

    // Check if the oldest unread target has scrolled above the viewport.
    if (_oldestAutoScrollTarget && _oldestAutoScrollTarget !== newEl) {
      const oldRect = _oldestAutoScrollTarget.getBoundingClientRect();
      if (oldRect.top < cRect.top) {
        scrollToMessage(_oldestAutoScrollTarget, 'instant', 'top');
        return;
      }
    }
    // Otherwise scroll to bottom — new messages are always last, and this
    // shows the container's padding-bottom as breathing room below the message.
    scrollChatToBottom();
  });
}

/** For user's own sent messages — always scroll to show it. */
export function handleSentMessageScroll(el) {
  _oldestAutoScrollTarget = null;
  if (_scrollRAF) cancelAnimationFrame(_scrollRAF);
  scrollChatToBottom();
}

// ---- New content bubbles ----

const newChatBubble = document.getElementById('new-chat-bubble');
const newActivityBubble = document.getElementById('new-activity-bubble');
let firstUnseenChatEl = null;
let firstUnseenActivityEl = null;

export function showChatBubble(el) {
  if (!firstUnseenChatEl) firstUnseenChatEl = el;
  newChatBubble.classList.add('visible');
  document.getElementById('chat-scroll-arrow')?.classList.remove('visible');
}
export function hideChatBubble() {
  newChatBubble.classList.remove('visible');
  firstUnseenChatEl = null;
  _oldestAutoScrollTarget = null;
}
export function showActivityBubble(el) {
  if (!firstUnseenActivityEl) firstUnseenActivityEl = el;
  newActivityBubble?.classList.add('visible');
  document.getElementById('activity-scroll-arrow')?.classList.remove('visible');
}
export function hideActivityBubble() {
  newActivityBubble?.classList.remove('visible');
  firstUnseenActivityEl = null;
}

newChatBubble?.addEventListener('click', () => {
  if (firstUnseenChatEl) {
    scrollToMessage(firstUnseenChatEl, 'smooth', 'top');
  } else {
    scrollChatToBottom();
  }
  hideChatBubble();
});

newActivityBubble?.addEventListener('click', () => {
  if (firstUnseenActivityEl) {
    firstUnseenActivityEl.scrollIntoView({ block: 'start', behavior: 'smooth' });
  } else {
    const body = document.querySelector('[data-console-panel="activity"]');
    if (body) body.scrollTop = body.scrollHeight;
  }
  hideActivityBubble();
});

// Scroll-to-bottom arrows (shown when scrolled up and no new-content bubble visible).
const chatScrollArrow = document.getElementById('chat-scroll-arrow');
const activityScrollArrow = document.getElementById('activity-scroll-arrow');

export function updateChatScrollArrow() {
  const nearBottom = isChatNearBottom();
  if (nearBottom) { hideChatBubble(); chatScrollArrow.classList.remove('visible'); return; }
  const bubbleVisible = newChatBubble.classList.contains('visible');
  chatScrollArrow.classList.toggle('visible', !bubbleVisible);
}
export function updateActivityScrollArrow() {
  if (!activityScrollArrow) return;
  const nearBottom = isConsoleNearBottom();
  if (nearBottom) { hideActivityBubble(); activityScrollArrow.classList.remove('visible'); return; }
  const bubbleVisible = newActivityBubble?.classList.contains('visible') || false;
  activityScrollArrow.classList.toggle('visible', !bubbleVisible);
}

// Code block wrap toggle (event delegation)
document.addEventListener('click', (e) => {
  const wrapBtn = e.target.closest('.md-code-wrap-toggle');
  if (wrapBtn) {
    const block = wrapBtn.closest('.md-code-block');
    if (block) block.classList.toggle('wrap-on');
    return;
  }
  const copyBtn = e.target.closest('.md-code-copy');
  if (copyBtn) {
    const block = copyBtn.closest('.md-code-block');
    if (block) {
      const code = block.querySelector('code');
      if (code) {
        navigator.clipboard.writeText(code.textContent).then(() => {
          copyBtn.textContent = 'copied!';
          setTimeout(() => { copyBtn.innerHTML = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="5" width="9" height="9" rx="1.5"/><path d="M5 11H3.5A1.5 1.5 0 012 9.5v-7A1.5 1.5 0 013.5 1h7A1.5 1.5 0 0112 2.5V5"/></svg>copy'; }, 2000);
        });
      }
    }
    return;
  }
  const embedWrapBtn = e.target.closest('.build-embed-wrap-toggle');
  if (embedWrapBtn) {
    e.stopPropagation(); // prevent toggle collapse
    const embed = embedWrapBtn.closest('.build-embed');
    if (embed) embed.classList.toggle('wrap-on');
    return;
  }
});

document.getElementById('chat-messages').addEventListener('scroll', updateChatScrollArrow);
document.querySelector('[data-console-panel="activity"]')?.addEventListener('scroll', updateActivityScrollArrow);

chatScrollArrow?.addEventListener('click', () => { scrollChatToBottom(); chatScrollArrow.classList.remove('visible'); });
activityScrollArrow?.addEventListener('click', () => {
  const body = document.querySelector('[data-console-panel="activity"]');
  if (body) body.scrollTop = body.scrollHeight;
  activityScrollArrow.classList.remove('visible');
});

export function scrollToFirstUnread(container) {
  if (!state.chatCurrentChannel) return false;
  // Use captured lastSeen from channel switch, falling back to live value.
  const lastSeen = state.scrollLastSeen || getLastSeen(state.chatCurrentChannel);
  if (!lastSeen) return false;
  const msgEls = container.querySelectorAll('.msg[data-created-at]');
  for (const el of msgEls) {
    if (el.dataset.createdAt > lastSeen) {
      scrollToMessage(el, 'instant', 'top');
      state.scrollLastSeen = null; // consumed
      return true;
    }
  }
  return false;
}

export function createEmptyState(title, desc) {
  const div = document.createElement('div');
  div.className = 'empty-state';
  div.innerHTML = `
    <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
      <path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z"/>
    </svg>
    <h3>${title}</h3>
    <p>${desc}</p>
  `;
  return div;
}
