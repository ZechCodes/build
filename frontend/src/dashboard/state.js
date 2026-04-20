// Shared, mutable dashboard state.
//
// This single object holds every cross-module binding the dashboard relies on.
// Modules read and mutate via `state.xxx = ...` (primitives) or by mutating
// the contained Maps/Sets/Arrays in place.
//
// The shape is intentionally flat: a rename is a find-and-replace rather than
// a chain of module imports. Feature-local state lives inside the feature's
// own module, not here.

export const state = {
  // ----- Shell / UI -----
  currentTab: 'files',
  selectedAgent: null,
  consoleState: 'collapsed', // 'collapsed' | 'open' | 'expanded'
  deviceDown: null,

  // ----- Devices -----
  devices: new Map(),
  activityItems: [],

  // ----- Dropdowns / overlays -----
  _openDropdown: null,

  // ----- Tasks -----
  channelTodos: new Map(),

  // ----- E2EE -----
  e2eeHasConnected: false,
  _reconnectPillTimer: null,
  e2eeConnections: new Map(),
  channelDeviceMap: new Map(),
  deviceChannels: new Map(),
  deviceHarnesses: new Map(),

  // ----- Channels -----
  chatCurrentChannel: null,
  chatChannels: new Map(),
  chatMessages: new Map(),
  unreadCounts: new Map(),
  channelLastSeen: new Map(),
  channelSortTs: new Map(),
  scrollLastSeen: null,
  _unreadHighlightUntil: null,
  _unreadHighlightLastSeen: null,
  channelHistory: [],
  channelHistoryIndex: -1,
  _navigatingHistory: false,
  channelAgentActive: new Map(),
  chatLoadingMessages: new Set(),
  chatLoadingActivity: new Set(),
  channelPlanMode: new Map(),

  // ----- Browser tabs -----
  browserTabs: new Map(),
  activeBrowserTab: null,
  _browserTabCounter: 0,

  // ----- Terminal -----
  deviceAgentCwd: new Map(),
  terminalCwdMap: new Map(),
  terminalHistoryMap: new Map(),
  terminalCmdHistory: [],
  terminalCmdIndex: -1,
  terminalRunning: false,
  terminalCurrentBlock: null,
  terminalCompletionPending: false,
  terminalCompletions: [],
  terminalCompletionIndex: -1,
  terminalCompletionBase: '',
  terminalCompletionPartial: '',
  terminalLoadingTimer: null,
  terminalLoadingInterval: null,
  terminalNoOutputTimer: null,
  terminalKillTimer: null,

  // ----- Complications -----
  complicationState: new Map(),
  compPopoverOpen: null,

  // ----- Chat composer -----
  pendingFiles: [],

  // ----- Files view -----
  fileTreeData: new Map(),
  filesCurrentPath: null,
  filesCurrentView: 'source',
  filesChannelId: null,
  filesCurrentHasDiff: false,
  filesCurrentIsMarkdown: false,
  filesCurrentIsSvg: false,
  filesCurrentIsHtml: false,
  filesLastContent: null,
  filesPendingRestore: null,
  filesPendingView: 'source',
  filesTreeTab: 'changes',
  filesChangesData: new Map(),
  _filesSelfHealed: false,
  filesLineWrap: localStorage.getItem('filesLineWrap') === '1',
};

// Constants: not mutable, but commonly referenced — export alongside.
export const MAX_ACTIVITY = 50;
export const SORT_STABILITY_MS = 60 * 60 * 1000; // 1 hour
export const CHANNEL_STATE_KEY = 'build_channel_state';
export const THEME_VERSION_KEY = 'build_theme_version';
export const MAX_FILE_SIZE = 50 * 1024 * 1024;
export const CHAT_OVERLAY_STATE_KEY = 'chat.overlay.state';
export const CONSOLE_RECENT_COUNT = 30;
