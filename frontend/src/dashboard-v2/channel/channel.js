// One instance per opened channel. Owns per-channel view state and the
// subviews that render inside that channel's DOM slots.
// See planning/dashboard-v2/03-channel-lifecycle.md.

import { bus } from '../core/bus.js';
import { messagesStore } from '../domain/messages-store.js';
import { unreadStore } from '../domain/unread-store.js';
import { channelsStore } from '../domain/channels-store.js';
import { ChatView } from './views/chat-view.js';
import { ConsoleView } from './views/console-view.js';
import { FilesView } from './views/files-view.js';
import { TerminalView } from './views/terminal-view.js';
import { log } from '../core/log.js';

const plog = log('channel');

function defaultViewState() {
  return {
    // Chat
    draftText: '',
    scrollAnchor: null,
    unreadHighlightLastSeen: null,
    consoleScrollNearBottom: true,
    pendingFiles: [],   // File[] staged in composer, not yet uploaded
    // Files
    filesPath: null,
    filesView: 'source',           // 'source' | 'diff' | 'preview'
    filesLineWrap: false,
    filesTreeTab: 'changes',       // 'changes' | 'all'
    filesExpandedDirs: [],         // list of dir paths the user has expanded
    filesTreeScrollTop: 0,
    filesViewerScrollTop: 0,
    // Terminal
    terminalCwd: null,
    terminalCmdHistory: [],
    terminalCmdIndex: -1,
    terminalRunning: false,
    terminalCompletionBase: '',
    terminalCompletionPartial: '',
    terminalCompletions: [],
    terminalCompletionIndex: -1,
    // Meta
    lastActivatedAt: 0,
  };
}

// ── viewState persistence ────────────────────────────────────────────
// Small localStorage bucket per channel so "leave and come back"
// (switching channels OR reloading the page) restores the user's
// file-tree + viewer workspace — which tab, which file, expanded
// dirs, scroll positions, line-wrap, etc.
//
// Chat / terminal / console state is not in the persisted subset
// by design: draft text is view-private, terminal history is
// session-y, and console is just an activity feed.

const PERSIST_VERSION = 1;
const PERSIST_KEY = (id) => `v2:channel:${id}:viewState`;
const PERSIST_FIELDS = [
  'filesPath', 'filesView', 'filesLineWrap', 'filesTreeTab',
  'filesExpandedDirs', 'filesTreeScrollTop', 'filesViewerScrollTop',
];

function loadPersisted(id) {
  if (typeof localStorage === 'undefined') return null;
  try {
    const raw = localStorage.getItem(PERSIST_KEY(id));
    if (!raw) return null;
    const data = JSON.parse(raw);
    if (data?._v !== PERSIST_VERSION) return null;
    return data;
  } catch { return null; }
}

function savePersisted(id, viewState) {
  if (typeof localStorage === 'undefined') return;
  try {
    const snap = { _v: PERSIST_VERSION };
    for (const f of PERSIST_FIELDS) snap[f] = viewState[f];
    localStorage.setItem(PERSIST_KEY(id), JSON.stringify(snap));
  } catch { /* quota / private mode — ignore */ }
}

export class Channel {
  constructor(id, { views } = {}) {
    this.id = id;
    this.viewState = defaultViewState();
    // Restore the persisted subset (files tab, expanded dirs, scroll,
    // etc.) so the user's workspace comes back intact across channel
    // switches and page reloads.
    const persisted = loadPersisted(id);
    if (persisted) {
      for (const f of PERSIST_FIELDS) {
        if (f in persisted) this.viewState[f] = persisted[f];
      }
    }
    // Test hook: views can be injected.
    if (views) {
      this.views = views;
    } else {
      this.views = {
        chat: new ChatView(this),
        console: new ConsoleView(this),
        files: new FilesView(this),
        terminal: new TerminalView(this),
      };
    }
    this._active = false;
    this._unsubs = [];
    this._fetchedOnce = false;
  }

  activate() {
    if (this._active) return;
    this._active = true;
    this.viewState.lastActivatedAt = Date.now();
    plog.debug('activate', this.id);

    for (const v of Object.values(this.views)) v.activate();

    // Initial fetch attempt — may no-op if transport isn't up yet.
    this._tryFetchHistory();

    // Re-fetch when the channel becomes routable after initial load:
    //   - e2ee.connected for its device
    //   - channel.list / channel.upserted lands with this channel id
    this._unsubs.push(bus.on('e2ee.connected', () => this._tryFetchHistory()));
    this._unsubs.push(channelsStore.subscribe(e => {
      if (e.kind === 'replace_for_device' || e.id === this.id) {
        this._tryFetchHistory();
      }
    }));

    // Mark unread messages read. We use the msg list we have;
    // the intent handler accepts ids it doesn't recognize as a no-op.
    const unread = unreadStore.get(this.id);
    if (unread.count > 0) {
      const ids = messagesStore.forChannel(this.id)
        .filter(m => m.sender !== 'client' && !m.read_at && m.id)
        .map(m => m.id);
      if (ids.length) bus.emit('intent.mark_read', { channelId: this.id, msgIds: ids });
      unreadStore.markRead(this.id);
    }
    bus.emit('intent.mark_seen', { channelId: this.id });
  }

  _tryFetchHistory() {
    if (!this._active) return;
    // Only fetch once the channel resolves to a device (meaning the
    // transport has picked it up).
    if (!channelsStore.deviceFor(this.id)) return;
    if (this._fetchedOnce && messagesStore.forChannel(this.id).length > 0) return;
    this._fetchedOnce = true;
    if (messagesStore.forChannel(this.id).length === 0) {
      bus.emit('intent.get_messages', { channelId: this.id });
    }
    bus.emit('intent.get_activity', { channelId: this.id });
    bus.emit('intent.get_complications', { channelId: this.id });
  }

  deactivate() {
    if (!this._active) return;
    this._active = false;
    plog.debug('deactivate', this.id);
    this._unsubs.forEach(fn => fn());
    this._unsubs = [];
    this._fetchedOnce = false;
    for (const v of Object.values(this.views)) v.deactivate();
    // Snapshot the persist subset — captures whatever the views just
    // wrote onto viewState during their own deactivate() paths (scroll
    // positions in particular).
    savePersisted(this.id, this.viewState);
  }

  /** Persist the current viewState subset immediately — used by the
   *  beforeunload handler so page reloads don't lose state on the
   *  active channel (which doesn't go through deactivate in that
   *  path). */
  persistNow() {
    if (!this.viewState) return;
    savePersisted(this.id, this.viewState);
  }

  destroy() {
    this.deactivate();
    this.viewState = null;
    this.views = null;
  }

  get active() { return this._active; }
}
