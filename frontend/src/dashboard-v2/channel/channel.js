// One instance per opened channel. Owns per-channel view state and the
// subviews that render inside that channel's DOM slots.
//
// Lifecycle:
//   unmounted → mounting → mounted → loading → ready
//                                         ↘              ↙
//                                           unloading → unmounted
//
// mount()  — synchronous. DOM + view subscriptions. Does not fetch.
// load()   — async. Awaits sessionStore.awaitChannelReady(id, { signal })
//            then emits the initial-data intents. Transitions to `ready`
//            after intents are dispatched; individual store updates
//            arrive as responses stream in.
// unload() — async. Aborts any in-flight load, unmounts views,
//            unsubscribes channel-level bus listeners, persists the
//            localStorage subset of viewState.
// destroy() — unload + release viewState / views. Terminal.
//
// The shell's registry drives this. Views keep their activate()/
// deactivate() names for back-compat with shell-view conventions.

import { bus } from '../core/bus.js';
import { messagesStore } from '../domain/messages-store.js';
import { unreadStore } from '../domain/unread-store.js';
import { loadChannel } from '../transport/channel-loader.js';
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
    const persisted = loadPersisted(id);
    if (persisted) {
      for (const f of PERSIST_FIELDS) {
        if (f in persisted) this.viewState[f] = persisted[f];
      }
    }
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
    this._phase = 'unmounted';
    this._subs = new Set();
    this._loadAC = null;
    this._loadPromise = null;
    // How long load() waits for the first responses to land before it
    // declares `ready` anyway. Prevents a lost response from stranding
    // the UI in `loading` indefinitely. Tunable for tests.
    this._loadTimeoutMs = 5000;
  }

  get phase() { return this._phase; }

  // Back-compat accessor for older call sites that just need "is it
  // mounted?"; anything new should read `phase` instead.
  get active() { return this._phase !== 'unmounted' && this._phase !== 'unloading'; }

  subscribe(fn) {
    this._subs.add(fn);
    try { fn({ phase: this._phase, prevPhase: this._phase }); }
    catch (err) { plog.error('channel subscriber threw on subscribe', err); }
    return () => this._subs.delete(fn);
  }

  _setPhase(next) {
    if (this._phase === next) return;
    const prev = this._phase;
    this._phase = next;
    plog.debug(this.id, 'phase', prev, '→', next);
    for (const fn of this._subs) {
      try { fn({ phase: next, prevPhase: prev }); }
      catch (err) { plog.error('channel subscriber threw', err); }
    }
    bus.emit('channel.phase', { channelId: this.id, phase: next, prevPhase: prev });
  }

  /** Synchronous DOM + subscription binding. Idempotent. */
  mount() {
    if (this._phase !== 'unmounted') return;
    this._setPhase('mounting');
    this.viewState.lastActivatedAt = Date.now();
    for (const v of Object.values(this.views)) v.activate();
    this._setPhase('mounted');

    // Mark-read is a side-effect of mount, not of load — we want the
    // unread count to clear as soon as the user views the channel,
    // regardless of whether the historical fetch has rendered yet.
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

  /**
   * Wait for the session to be ready for this channel's device, then
   * fire the initial-data intents. Resolves when intents are dispatched.
   * Cancellable — unload() aborts an in-flight load.
   *
   * Idempotent: concurrent callers share the same promise.
   */
  load() {
    if (this._phase === 'unmounted') {
      return Promise.reject(new Error('load() before mount()'));
    }
    if (this._loadPromise) return this._loadPromise;

    const ac = this._loadAC = new AbortController();
    this._setPhase('loading');

    const run = async () => {
      try {
        await loadChannel(this.id, {
          signal: ac.signal,
          timeoutMs: this._loadTimeoutMs,
        });
        if (ac.signal.aborted) return;
        this._setPhase('ready');
      } catch (err) {
        if (err?.name !== 'AbortError') plog.error('load failed', this.id, err);
        // On abort, unload() owns the phase transition — don't step on it.
        throw err;
      } finally {
        this._loadPromise = null;
      }
    };
    this._loadPromise = run();
    return this._loadPromise;
  }

  /**
   * Abort in-flight load, unmount views, unsubscribe, persist.
   * Idempotent.
   */
  async unload() {
    if (this._phase === 'unmounted') return;
    const inFlight = this._loadPromise;
    this._loadAC?.abort();
    this._setPhase('unloading');

    // Wait for the aborted load to settle so we don't race its
    // final bus emits with teardown.
    if (inFlight) {
      try { await inFlight; }
      catch (_) { /* expected AbortError */ }
    }

    for (const v of Object.values(this.views)) {
      try { v.deactivate(); }
      catch (err) { plog.error('view deactivate threw', err); }
    }

    savePersisted(this.id, this.viewState);

    this._loadAC = null;
    this._loadPromise = null;
    this._setPhase('unmounted');
  }

  /** For beforeunload — persist immediately without changing phase. */
  persistNow() {
    if (!this.viewState) return;
    savePersisted(this.id, this.viewState);
  }

  async destroy() {
    await this.unload();
    this.viewState = null;
    this.views = null;
    this._subs.clear();
  }

  // ── back-compat shims ──────────────────────────────────────────────
  // Older tests (and the shell before Phase 2 migrated) call
  // activate() / deactivate() directly. These forward so existing code
  // keeps working. Prefer mount() + load() / unload() in new call sites.
  activate() {
    this.mount();
    // Fire-and-forget: the promise resolves asynchronously; old callers
    // don't await it. Swallow abort.
    this.load().catch(err => {
      if (err?.name !== 'AbortError') plog.error('load failed', this.id, err);
    });
  }

  deactivate() {
    // Sync legacy API — fire-and-forget the async unload.
    this.unload();
  }
}

