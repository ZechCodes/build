import { state } from '../state.js';
import { getActiveE2EE } from '../e2ee/bridge.js';
import { saveChannelState } from '../channels/state-store.js';

// External deps via window during transition:
//   window.updatePlanModeUI (moves to chat/interactions.js in P3)

// Show/hide commands tray based on input focus. Uses a short delay on blur
// so that clicks/taps on tray buttons have time to fire before it hides.
const cmdTray = document.getElementById('commands-tray');
const chatInputArea = document.querySelector('.chat-input-area');
let trayHideTimeout = null;
chatInputArea?.addEventListener('focusin', () => {
  clearTimeout(trayHideTimeout);
  cmdTray?.classList.add('focus-visible');
});
chatInputArea?.addEventListener('focusout', () => {
  trayHideTimeout = setTimeout(() => cmdTray?.classList.remove('focus-visible'), 200);
});
// Refocus the input after any tray button click so the tray stays visible.
cmdTray?.addEventListener('click', () => {
  document.getElementById('chat-input')?.focus();
});

// Attach button (in tray)
document.getElementById('cmd-attach-btn')?.addEventListener('click', () => {
  document.getElementById('chat-file-input').click();
});

// Plan button (in tray)
document.getElementById('cmd-plan-btn')?.addEventListener('click', () => {
  if (!state.chatCurrentChannel) return;
  const current = state.channelPlanMode.get(state.chatCurrentChannel) || false;
  state.channelPlanMode.set(state.chatCurrentChannel, !current);
  saveChannelState(state.chatCurrentChannel, { planMode: !current });
  window.updatePlanModeUI?.(!current);
});

// Compact button (in tray) — double-click confirm pattern.
let compactConfirmTimeout = null;
document.getElementById('cmd-compact-btn')?.addEventListener('click', () => {
  const btn = document.getElementById('cmd-compact-btn');
  if (!state.chatCurrentChannel) return;
  if (btn.classList.contains('confirm')) {
    clearTimeout(compactConfirmTimeout);
    btn.classList.remove('confirm');
    const label = btn.querySelector('span');
    if (label) label.textContent = 'Compact';
    getActiveE2EE()?.compactSession(state.chatCurrentChannel);
  } else {
    btn.classList.add('confirm');
    const label = btn.querySelector('span');
    if (label) label.textContent = 'Click to confirm';
    compactConfirmTimeout = setTimeout(() => {
      btn.classList.remove('confirm');
      const label = btn.querySelector('span');
      if (label) label.textContent = 'Compact';
    }, 3000);
  }
});

// Clear button (in tray) — double-click confirm pattern.
let resetConfirmTimeout = null;
document.getElementById('cmd-reset-btn')?.addEventListener('click', () => {
  const btn = document.getElementById('cmd-reset-btn');
  if (!state.chatCurrentChannel) return;
  if (btn.classList.contains('confirm')) {
    clearTimeout(resetConfirmTimeout);
    btn.classList.remove('confirm');
    const label = btn.querySelector('span');
    if (label) label.textContent = 'Clear';
    getActiveE2EE()?.resetSession(state.chatCurrentChannel);
  } else {
    btn.classList.add('confirm');
    const label = btn.querySelector('span');
    if (label) label.textContent = 'Click to confirm';
    resetConfirmTimeout = setTimeout(() => {
      btn.classList.remove('confirm');
      const label = btn.querySelector('span');
      if (label) label.textContent = 'Clear';
    }, 3000);
  }
});
