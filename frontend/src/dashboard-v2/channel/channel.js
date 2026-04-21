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

export class Channel {
  constructor(id, { views } = {}) {
    this.id = id;
    this.viewState = defaultViewState();
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
  }

  destroy() {
    this.deactivate();
    this.viewState = null;
    this.views = null;
  }

  get active() { return this._active; }
}
