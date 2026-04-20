import { state, CONSOLE_RECENT_COUNT } from '../state.js';
import { fmtRelativeAgo, fmtClock24 } from '../util/time.js';
import { toggleChatOverlay } from '../chat/overlay.js';

const consoleBtm = document.getElementById('console-bottom');
const consoleToggle = document.getElementById('console-toggle');
const consoleExpand = document.getElementById('console-expand');

export function setConsoleState(next) {
  state.consoleState = next;
  const isCollapsed = next === 'collapsed';
  consoleBtm.classList.toggle('collapsed', isCollapsed);
  consoleBtm.classList.toggle('rail-only', isCollapsed);
  consoleBtm.classList.toggle('expanded', next === 'expanded');
  const term = document.querySelector('[data-console-panel="terminal"]');
  if (term) term.classList.toggle('hidden', isCollapsed);
  updateConsoleButtons();
  updateRailButtonStates();
}

export function updateConsoleButtons() {
  const s = state.consoleState;
  const expandSvg = '<path d="M4 10L10 4M10 4H5M10 4v5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>';
  const restoreSvg = '<path d="M10 4L4 10M4 10h5M4 10V5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>';
  if (s === 'collapsed') {
    consoleExpand.classList.add('hidden');
    consoleToggle.classList.add('hidden');
  } else if (s === 'open') {
    consoleExpand.classList.remove('hidden');
    consoleToggle.classList.remove('hidden');
    consoleExpand.title = 'Expand terminal';
    consoleExpand.querySelector('svg').innerHTML = expandSvg;
    consoleExpand.onclick = () => setConsoleState('expanded');
    consoleToggle.title = 'Close terminal';
    consoleToggle.querySelector('svg').style.transform = '';
    consoleToggle.onclick = () => setConsoleState('collapsed');
  } else {
    consoleExpand.classList.remove('hidden');
    consoleToggle.classList.remove('hidden');
    consoleExpand.title = 'Restore terminal';
    consoleExpand.querySelector('svg').innerHTML = restoreSvg;
    consoleExpand.onclick = () => setConsoleState('open');
    consoleToggle.title = 'Close terminal';
    consoleToggle.querySelector('svg').style.transform = '';
    consoleToggle.onclick = () => setConsoleState('collapsed');
  }
}

export function updateRailButtonStates() {
  const tBtn = document.getElementById('console-terminal-toggle');
  if (tBtn) tBtn.classList.toggle('active', state.consoleState !== 'collapsed');
  const cBtn = document.getElementById('console-chat-toggle');
  const overlay = document.getElementById('chat-overlay');
  if (cBtn && overlay) cBtn.classList.toggle('active', overlay.classList.contains('open'));
}

export function updateChatRailUnreadBadge() {
  const badge = document.getElementById('chat-rail-badge');
  if (!badge) return;
  let total = 0;
  if (typeof state.unreadCounts !== 'undefined') {
    for (const [chId, uc] of state.unreadCounts) {
      if (chId === state.chatCurrentChannel) continue;
      total += (uc && uc.messages) || 0;
    }
  }
  if (total > 0) {
    badge.textContent = total > 99 ? '99+' : String(total);
    badge.hidden = false;
  } else {
    badge.textContent = '';
    badge.hidden = true;
  }
}

// Console-entry / tree expand toggles — DOM chrome shared with the bottom console.
export function toggleConsoleEntry(row) {
  const expandIcon = row.querySelector('.ce-expand');
  const detail = row.nextElementSibling;
  if (detail && detail.classList.contains('ce-detail')) {
    if (expandIcon) expandIcon.classList.toggle('open');
    detail.classList.toggle('open');
  }
}

export function toggleTree(el) {
  const children = el.nextElementSibling;
  if (children && children.classList.contains('tree-children')) {
    children.classList.toggle('collapsed');
    const arrow = el.querySelector('.tree-arrow');
    if (arrow) arrow.classList.toggle('open');
  }
}

// ===== Activity timestamps =====
// Last 30 entries show "Ns/Nm/Nh/Nd ago"; older entries show 24h "HH:MM".
export function renderConsoleTimes() {
  const body = document.querySelector('[data-console-panel="activity"]');
  if (!body) return;
  const entries = body.querySelectorAll('.console-entry');
  const total = entries.length;
  const recentFloor = Math.max(0, total - CONSOLE_RECENT_COUNT);
  const now = Date.now();
  entries.forEach((entry, idx) => {
    const el = entry.querySelector('.ce-time');
    if (!el) return;
    const ts = parseInt(el.dataset.ts || '0', 10);
    if (!ts) return;
    const when = new Date(ts);
    el.title = when.toLocaleString();
    el.textContent = idx >= recentFloor ? fmtRelativeAgo(now - ts) : fmtClock24(when);
  });
}

// Initial paint + interval tick
updateConsoleButtons();
updateRailButtonStates();
setInterval(renderConsoleTimes, 1000);

// Rail button wiring.
document.getElementById('console-terminal-toggle')?.addEventListener('click', (e) => {
  e.stopPropagation();
  if (state.consoleState === 'collapsed') {
    setConsoleState('open');
    document.getElementById('terminal-input')?.focus();
  } else {
    setConsoleState('collapsed');
  }
});
document.getElementById('console-chat-toggle')?.addEventListener('click', (e) => {
  e.stopPropagation();
  toggleChatOverlay();
});
