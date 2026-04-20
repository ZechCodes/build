import { state } from '../state.js';
import { getE2EE } from '../e2ee/bridge.js';
import {
  pushChannelHistory,
  getOrderedChannelIds,
  renderChannelList,
} from './list.js';
import { getLastSeen, setLastSeen, deferMarkRead } from './unread.js';
import { loadChannelState } from './state-store.js';
import { renderChannelPanel, updateMobileChannelLabel } from './panel.js';
import { syncChatOverlayHeader, applyChannelHarnessInfo } from '../chat/overlay.js';
import { hideChatBubble, hideActivityBubble, clearConsole } from '../console/view.js';
import { switchTab } from '../shell/tabs.js';
import { onFilesTabActivated } from '../files/view.js';
import { renderTerminalForChannel } from '../terminal/terminal.js';
import { renderTasksPanel, updateTasksBadge } from '../tasks/panel.js';
import { closeCompPopover, renderComplications } from '../complications/render.js';

// Functions still defined in legacy.js — bridged via window during the transition:
//   window.renderMessages, window.updateStopButton, window.updatePlanModeUI

export function selectChannel(channelId) {
  state.chatCurrentChannel = channelId;
  pushChannelHistory(channelId);
  state.activeBrowserTab = null;
  // Hide browser panel and restore normal tab if it was showing
  document.getElementById('tab-browser')?.classList.remove('active');
  // Restore viewer chrome hidden by browser view
  document.querySelector('.viewer-body')?.classList.remove('hidden');
  document.querySelector('.comp-wrapper')?.classList.remove('hidden');
  document.getElementById('console-bottom')?.classList.remove('hidden');
  // Restore files view as the main panel.
  const filesPanel = document.getElementById('tab-files');
  if (filesPanel) filesPanel.classList.add('active');
  // Sync the chat overlay header + model/effort pills for this channel.
  syncChatOverlayHeader();
  applyChannelHarnessInfo(channelId);
  // Close mobile channel panel if open
  document.getElementById('channel-panel')?.classList.remove('mobile-open');
  window.updateStopButton?.();
  hideChatBubble();
  hideActivityBubble();
  // Capture lastSeen for scroll positioning before updating it.
  state.scrollLastSeen = getLastSeen(channelId) || null;
  state._unreadHighlightLastSeen = null;
  state._unreadHighlightUntil = null;
  // Defer marking as seen until user interacts.
  const _chId = channelId;
  deferMarkRead(() => {
    setLastSeen(_chId);
    state.unreadCounts.delete(_chId);
    renderChannelList();
  });
  renderChannelList();
  const _selConn = getE2EE(channelId);
  if (_selConn && _selConn.connected) {
    state.chatLoadingMessages.add(channelId);
    state.chatLoadingActivity.add(channelId);
    _selConn.getMessages(channelId);
    _selConn.getActivity(channelId);
    _selConn.getComplications(channelId);
  } else if (!state.chatMessages.has(channelId) || state.chatMessages.get(channelId).length === 0) {
    // Device disconnected, no cached messages — show loading state until reconnect.
    state.chatLoadingMessages.add(channelId);
  }
  window.renderMessages?.();
  clearConsole(state.chatLoadingActivity.has(channelId));
  const chName = state.chatChannels.get(channelId)?.name || '';
  const input = document.getElementById('chat-input');
  if (input) input.placeholder = `Message #${chName}...`;
  // Update mobile channel label
  updateMobileChannelLabel();
  location.hash = `${state.currentTab || 'chat'}/${channelId}`;
  // If files tab is active, refresh it for the new channel.
  if (state.currentTab === 'files') onFilesTabActivated();
  // Restore persisted state from localStorage.
  const saved = loadChannelState(channelId);
  // Restore last active tab for this channel.
  if (saved.activeTab && saved.activeTab !== state.currentTab) {
    switchTab(saved.activeTab);
    if (saved.activeTab === 'files') onFilesTabActivated();
    location.hash = `${saved.activeTab}/${channelId}`;
  }
  if (input) {
    input.value = saved.draft || '';
    input.style.height = 'auto';
  }
  // Plan mode: prefer server state, fall back to localStorage.
  if (!state.channelPlanMode.has(channelId) && saved.planMode !== undefined) {
    state.channelPlanMode.set(channelId, saved.planMode);
  }
  window.updatePlanModeUI?.(state.channelPlanMode.get(channelId) || false);
  renderTerminalForChannel(channelId);
  renderTasksPanel(channelId);
  updateTasksBadge(channelId);
  closeCompPopover();
  renderComplications();
}

export function selectAgent(name, device) {
  state.selectedAgent = { name, device };
  renderChannelPanel();
  switchTab('chat');
  if (state.chatCurrentChannel) {
    location.hash = `chat/${state.chatCurrentChannel}`;
  } else {
    location.hash = 'chat';
  }
}

export function dismissStaleSuggestions(container, msgs) {
  // For each message with suggested_actions, check if a user message follows it.
  // If so, the suggestions are stale — dismiss them (and mark the matching one as selected).
  const suggestionDivs = container.querySelectorAll('.msg-suggestions');
  if (!suggestionDivs.length) return;

  // Build a map of msg index → next user message content (if any).
  const msgsWithSuggestions = [];
  for (let i = 0; i < msgs.length; i++) {
    if (msgs[i].suggested_actions?.length) {
      let nextUserContent = null;
      for (let j = i + 1; j < msgs.length; j++) {
        if (msgs[j].sender === 'client') {
          nextUserContent = msgs[j].content;
          break;
        }
      }
      msgsWithSuggestions.push({ msgIndex: i, nextUserContent, actions: msgs[i].suggested_actions });
    }
  }

  suggestionDivs.forEach((div, idx) => {
    const info = msgsWithSuggestions[idx];
    if (!info) return;
    if (info.nextUserContent !== null) {
      div.querySelectorAll('.suggestion-btn').forEach(btn => {
        if (btn.textContent === info.nextUserContent) {
          btn.classList.add('selected');
        } else {
          btn.classList.add('dismissed');
        }
      });
    }
  });
}

// Electron-only Alt+Up/Down/Shift+Up/Down/[/] channel navigation.
if (window.buildElectron) {
  document.addEventListener('keydown', (e) => {
    if (!e.altKey) return;

    if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      e.preventDefault();
      const ids = getOrderedChannelIds();
      if (!ids.length) return;
      const cur = ids.indexOf(state.chatCurrentChannel);

      if (e.shiftKey) {
        // ALT+SHIFT+UP/DOWN: jump to first unread in that direction
        const dir = e.key === 'ArrowUp' ? -1 : 1;
        const start = cur === -1 ? 0 : cur + dir;
        for (let i = start; i >= 0 && i < ids.length; i += dir) {
          const uc = state.unreadCounts.get(ids[i]);
          if (uc && uc.messages > 0) { selectChannel(ids[i]); return; }
        }
      } else {
        const next = e.key === 'ArrowUp' ? cur - 1 : cur + 1;
        if (next >= 0 && next < ids.length) selectChannel(ids[next]);
      }
    } else if (e.key === '[' || e.key === ']') {
      e.preventDefault();
      if (e.key === '[' && state.channelHistoryIndex > 0) {
        state.channelHistoryIndex--;
        state._navigatingHistory = true;
        selectChannel(state.channelHistory[state.channelHistoryIndex]);
        state._navigatingHistory = false;
      } else if (e.key === ']' && state.channelHistoryIndex < state.channelHistory.length - 1) {
        state.channelHistoryIndex++;
        state._navigatingHistory = true;
        selectChannel(state.channelHistory[state.channelHistoryIndex]);
        state._navigatingHistory = false;
      }
    }
  });
}
