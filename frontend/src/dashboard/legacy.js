import { escapeHtml, escHtml, formatBytes, formatFileSize } from './util/html.js';
import { timeAgo, shortTime, fmtRelativeAgo, fmtClock24 } from './util/time.js';
import { showToast } from './util/toast.js';
import { highlightLine } from './files/syntax.js';
import { effortLabel, modelToFriendlyName, compareVersions } from './util/format.js';
import { agentShortName, describeToolUse, formatToolDetail, formatToolResult, toolTag } from './console/tools.js';
import {
  state,
  MAX_ACTIVITY,
  SORT_STABILITY_MS,
  CHANNEL_STATE_KEY,
  THEME_VERSION_KEY,
  MAX_FILE_SIZE,
  CHAT_OVERLAY_STATE_KEY,
  CONSOLE_RECENT_COUNT,
} from './state.js';
import { closeAllDropdowns, positionDropdownMenu, toggleDropdown } from './shell/dropdown.js';
import { setSidebarSectionOpen, toggleSidebarSection, expandSidebarSection } from './shell/sidebar.js';
import {
  isChatOverlayOpen,
  openChatOverlay,
  closeChatOverlay,
  toggleChatOverlay,
  setChatOverlayPinned,
  toggleChatOverlayExpanded,
  syncChatOverlayHeader,
  applyChannelHarnessInfo,
} from './chat/overlay.js';
import './shell/update-badge.js';
import { renderTasksPanel, updateTasksBadge } from './tasks/panel.js';
import {
  getTerminalCwd,
  shortCwd,
  renderTerminalCwd,
  renderTerminalForChannel,
  terminalExec,
  clearTerminalTimers,
  finishTerminalCommand,
  clearTerminalCompletions,
} from './terminal/terminal.js';
import {
  renderComplications,
  renderGitCompChip,
  toggleCompPopover,
  closeCompPopover,
  sendComplicationAction,
} from './complications/render.js';
import {
  addBrowserTab,
  removeBrowserTab,
  selectBrowserTab,
  createBrowserTabItem,
} from './browser/tabs.js';
import {
  filesLoadRoot,
  renderFileTree,
  renderTreeLevel,
  renderChangesView,
  toggleDirectory,
  selectFile,
  setFilesMode,
  updateFloatingToggle,
  updateFilesModifiedCount,
  onFilesTabActivated,
  renderFileContent,
  renderPreview,
  renderMarkdownFile,
  renderSvgPreview,
  readFileAsync,
  resolveAssetPath,
  urlFetchAsync,
  resolveUrl,
  showBrowserView,
  renderDiffContent,
  onAgentFileChanges,
  navigateToFile,
} from './files/view.js';
import { renderChannelPanel, createChannelItem, updateMobileChannelLabel } from './channels/panel.js';
import { showEditChannelDialog } from './channels/edit-dialog.js';
import { renderDeviceCard } from './devices/render.js';
import { showEditDeviceDialog } from './devices/edit-dialog.js';
import { initNotifications, sseSynced } from './net/notifications.js';
import { getE2EE, getActiveE2EE, anyE2EEConnected } from './e2ee/bridge.js';
import { fileContentBody } from './files/view.js';
import { switchTab } from './shell/tabs.js';
import {
  setConsoleState,
  updateConsoleButtons,
  updateRailButtonStates,
  updateChatRailUnreadBadge,
  renderConsoleTimes,
  toggleConsoleEntry,
  toggleTree,
} from './shell/rail.js';
import './shell/update-badge.js';
import './chat/input.js';
import {
  addPendingFiles,
  clearPendingFiles,
  renderPendingFiles,
  _bindUploadProgress,
} from './chat/uploads.js';
import {
  rebuildChatChannels,
  promoteChannel,
  getOrderedChannelIds,
  pushChannelHistory,
  renderChannelList,
  renderChannelSidebar,
  updateAggregateBadge,
  incrementUnread,
} from './channels/list.js';
import {
  getLastSeen,
  setLastSeen,
  deferMarkRead,
  markUnreadMessages,
  clearUnreadHighlights,
} from './channels/unread.js';
import {
  loadChannelState,
  saveChannelState,
  clearChannelDraft,
} from './channels/state-store.js';
import { selectChannel, selectAgent, dismissStaleSuggestions } from './channels/select.js';
import { sendChatMessage } from './chat/composer.js';
import './chat/new-channel-dialog.js';
import {
  renderMessages,
  appendMessage,
  appendSystemMessage,
  updateStopButton,
} from './chat/messages.js';
import {
  appendInteractionCard,
  appendPlanReviewCard,
  resolvePendingPlanReviews,
  respondToInteraction,
  crossfadeStatus,
  respondToInteractionFreeform,
  respondToMultiselectInteraction,
  updatePlanModeUI,
} from './chat/interactions.js';
import { syncE2EEStatus, connectDeviceE2EE, initE2EE } from './e2ee/connect.js';
import { bindE2EEEvents } from './e2ee/handlers.js';
import {
  clearConsole,
  appendConsoleReasoning,
  appendConsoleEntry,
  markConsoleEntryDone,
  isChatNearBottom,
  isConsoleNearBottom,
  scrollChatToBottom,
  scrollToMessage,
  handleNewMessageScroll,
  handleSentMessageScroll,
  showChatBubble,
  hideChatBubble,
  showActivityBubble,
  hideActivityBubble,
  updateChatScrollArrow,
  updateActivityScrollArrow,
  scrollToFirstUnread,
  createEmptyState,
} from './console/view.js';

// App init runs after all feature modules have wired up their listeners.
import './app/notifications.js';
import './app/init.js';

// Electron detection
if (window.buildElectron) {
  document.body.classList.add('electron');
}

// ---- Expose helpers needed by vendor/markdown.js + window-bridged modules ----
window.highlightLine = highlightLine;

// `files/view.js` still uses window.switchTab + window.renderMessages because
// those modules pre-date P3 wave A/B and bridging is cheaper than untangling
// their import order.
window.switchTab = switchTab;
window.renderMessages = renderMessages;

// ---- Test bridges (Playwright smoke harness only) ----
// Used by frontend/tests/dashboard.smoke.mjs to inject a fake E2EE instance
// and exercise every bindE2EEEvents listener with synthetic envelopes.
window.__test_state = state;
window.__test_bindE2EEEvents = bindE2EEEvents;
