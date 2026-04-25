# 03 — Channel Lifecycle

A channel in v2 is a first-class object with a defined lifecycle. This is
the single biggest departure from v1, where "channel" was just an id threaded
through a pile of global Maps.

## Lifecycle states

```
 created  ─┬──► activated ─┬──► deactivated ─┬──► destroyed
           │               │                 │
           │               └────► re-activated
           │                                 ▲
           └─────────────────────────────────┘
```

- **created**: registry allocated the `Channel`, view state initialized,
  views instantiated but not mounted. No DOM, no subscriptions.
- **activated**: views mounted, subscriptions registered, initial data
  requested from transport.
- **deactivated**: views unmounted, subscriptions dropped. `viewState`
  persists (draft, scroll anchor, etc.).
- **destroyed**: `viewState` released. Channel leaves the registry.

## The `Channel` class

```js
// channel/channel.js
import { messagesStore } from '../domain/messages-store.js';
import { ChatView } from './views/chat-view.js';
import { FilesView } from './views/files-view.js';
import { TerminalView } from './views/terminal-view.js';
import { TasksView } from './views/tasks-view.js';
import { ConsoleView } from './views/console-view.js';
import { transport } from '../transport/index.js';

export class Channel {
  constructor(id) {
    this.id = id;
    this.hostEl = null;         // set on activate
    this.unsubs = [];
    this.viewState = {
      // Files
      filesPath: null,
      filesView: 'source',
      filesLineWrap: false,
      // Terminal
      terminalCwd: null,
      terminalBuffer: [],
      terminalRunning: false,
      terminalCmdHistory: [],
      terminalCmdIndex: -1,
      // Chat / Console
      scrollAnchor: null,
      unreadHighlightLastSeen: null,
      draftText: '',
      // Presence
      planMode: false,
    };
    this.views = {
      chat: new ChatView(this),
      files: new FilesView(this),
      terminal: new TerminalView(this),
      tasks: new TasksView(this),
      console: new ConsoleView(this),
    };
  }

  activate(hostEl) {
    if (this.hostEl) return;        // already active
    this.hostEl = hostEl;

    for (const v of Object.values(this.views)) v.activate();

    // Request initial data
    const conn = transport.e2ee.forChannel(this.id);
    if (conn?.connected) {
      conn.getMessages(this.id);
      conn.getActivity(this.id);
      conn.getComplications(this.id);
    }
  }

  deactivate() {
    if (!this.hostEl) return;       // already inactive

    for (const v of Object.values(this.views)) v.deactivate();

    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    this.hostEl = null;
  }

  destroy() {
    this.deactivate();
    this.viewState = null;
    this.views = null;
  }
}
```

## `ChannelRegistry`

One registry for the whole app. Keeps a pool of `Channel` instances, at most
one activated at a time.

```js
// channel/registry.js
import { Channel } from './channel.js';

class ChannelRegistry {
  constructor() {
    this.pool = new Map();       // channelId → Channel
    this.active = null;          // currently-activated Channel
    this.host = null;            // DOM mount point (set once)
  }

  bind(hostEl) { this.host = hostEl; }

  activate(channelId) {
    if (this.active?.id === channelId) return;

    if (this.active) this.active.deactivate();

    let ch = this.pool.get(channelId);
    if (!ch) {
      ch = new Channel(channelId);
      this.pool.set(channelId, ch);
    }
    ch.activate(this.host);
    this.active = ch;
  }

  deactivateAll() {
    if (this.active) this.active.deactivate();
    this.active = null;
  }

  evict(channelId) {
    const ch = this.pool.get(channelId);
    if (!ch) return;
    if (this.active === ch) this.active = null;
    ch.destroy();
    this.pool.delete(channelId);
  }
}

export const channelRegistry = new ChannelRegistry();
```

## When channels are created and destroyed

- **Created**: lazily on first `activate(channelId)`. No pre-allocation.
- **Destroyed**: when the channel is removed upstream (`channel.removed`
  bus event) OR when an LRU cap is hit (cap = 16 by default).
- **Eviction policy**: LRU by `viewState.lastActivatedAt`. The active
  channel is never evicted.

## What lives on `Channel.viewState`

Anything that should reset when you close and re-open the channel, or that
is meaningfully per-channel:

- File browser state (path, view mode, scroll, line-wrap toggle)
- Terminal buffer, cwd, command history, completion cursor
- Chat draft text
- Chat scroll anchor and unread-highlight cursor
- Plan mode toggle (server-authoritative; cached here for instant UI)
- Browser tabs (one logical browser per channel, not per device — revisit
  if spec says otherwise)

**What does NOT live on `viewState`**:

- Messages, activity entries, files tree data, task todos: these are
  domain data, owned by their respective stores and indexed by channelId.
  `viewState` holds *view-adjacent* state.

## Why this fixes v1 symptoms

- **Scroll drift between channels**: scroll anchor is per-`ConsoleView`
  instance, which is per-channel. Switching channels swaps instances.
- **`_filesSelfHealed` race**: `filesPath` is set by the `FilesView` when
  the channel activates. No global to race on.
- **Terminal completion stale**: completion cursor lives on `viewState`.
  The view re-renders from state on activate.
- **Channel history navigation**: router stack holds channelIds; registry
  activates on pop.

## Testing channel lifecycle

```js
test('activate/deactivate leaves no subscriptions', () => {
  const size = () => messagesStore._subscribers.size;
  const base = size();
  channelRegistry.activate('ch1');
  channelRegistry.activate('ch2');
  channelRegistry.deactivateAll();
  expect(size()).toBe(base);   // no leaks
});
```
