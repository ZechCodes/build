// One instance per opened channel. Owns per-channel view state and the
// subviews that render inside that channel's DOM slots.
// See planning/dashboard-v2/03-channel-lifecycle.md.

import { bus } from '../core/bus.js';
import { messagesStore } from '../domain/messages-store.js';
import { unreadStore } from '../domain/unread-store.js';
import { ChatView } from './views/chat-view.js';
import { ConsoleView } from './views/console-view.js';
import { log } from '../core/log.js';

const plog = log('channel');

function defaultViewState() {
  return {
    draftText: '',
    scrollAnchor: null,
    unreadHighlightLastSeen: null,
    consoleScrollNearBottom: true,
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
      };
    }
    this._active = false;
  }

  activate() {
    if (this._active) return;
    this._active = true;
    this.viewState.lastActivatedAt = Date.now();
    plog.debug('activate', this.id);

    for (const v of Object.values(this.views)) v.activate();

    // Fetch history if empty.
    if (messagesStore.forChannel(this.id).length === 0) {
      bus.emit('intent.get_messages', { channelId: this.id });
    }
    bus.emit('intent.get_activity', { channelId: this.id });
    bus.emit('intent.get_complications', { channelId: this.id });

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

  deactivate() {
    if (!this._active) return;
    this._active = false;
    plog.debug('deactivate', this.id);
    for (const v of Object.values(this.views)) v.deactivate();
  }

  destroy() {
    this.deactivate();
    this.viewState = null;
    this.views = null;
  }

  get active() { return this._active; }
}
