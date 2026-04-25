# 01 — Architecture

## Four layers

```
┌──────────────────────────────────────────────────────────┐
│  SHELL        layout, routing, tabs, rail, dropdowns     │
│               knows: router + active channel id          │
│               no domain logic                            │
├──────────────────────────────────────────────────────────┤
│  CHANNEL      one instance per open channel              │
│  CONTROLLER   owns per-channel view state                │
│               mounts chat/files/terminal/tasks subviews  │
│               lifecycle: create → activate → deactivate  │
├──────────────────────────────────────────────────────────┤
│  DOMAIN       stores (slices) + event bus                │
│  STORES       devices, channels, messages, activity,     │
│               files, terminal, tasks, complications,     │
│               presence, ui                               │
├──────────────────────────────────────────────────────────┤
│  TRANSPORT    E2EE, SSE, REST, upload                    │
│               publishes typed events; no DOM, no render  │
└──────────────────────────────────────────────────────────┘
```

## Rules between layers

These are the only inter-layer calls permitted. Violations are bugs.

1. **Transport never touches DOM or view state.** It dispatches
   `{type, payload}` events onto the bus.
2. **Stores never touch DOM.** They mutate their slice and notify
   subscribers. Stores may subscribe to the bus; views may subscribe to
   stores.
3. **Stores do not call other stores.** Cross-store effects go through the
   bus. If `messagesStore` wants `unreadStore` to update, it emits
   `message.received`; `unreadStore` subscribes.
4. **Channel controllers own per-channel view state.** Anything that should
   reset on channel switch lives on the `Channel` instance, not in a store.
5. **Views own their DOM, subscribe to stores, dispatch actions via the
   bus or store methods.** Views do not mutate stores directly except
   through the store's public methods.
6. **Shell never reaches into channels.** It emits `navigate(tab, channelId)`;
   the `ChannelRegistry` handles activation.
7. **No `window.*` bridges.** If two modules need to talk, import.

## Key abstractions

### Event bus (`core/bus.js`)

Typed pub/sub. No DOM, no state. Transport and stores communicate here.

```js
export const bus = {
  on(type, fn) { /* add listener, return unsubscribe */ },
  emit(type, payload) { /* sync dispatch */ },
};
```

Event types live in [04-transport.md](04-transport.md). They are domain
events (`message.received`, `channel.upserted`, `agent.tool_use`), not wire
events (`agent_event`).

### Store (`domain/<name>-store.js`)

Owns one slice. Mutation methods emit **changes** via an internal
subscription; subscribers react. Stores may also subscribe to the bus.

```js
// domain/messages-store.js
const byChannel = new Map();   // channelId → Message[]
const subs = new Set();

export const messagesStore = {
  forChannel(id) { return byChannel.get(id) ?? []; },
  append(channelId, msg) {
    const arr = byChannel.get(channelId) ?? [];
    arr.push(msg);
    byChannel.set(channelId, arr);
    notify({ channelId, msg });
  },
  subscribe(fn) { subs.add(fn); return () => subs.delete(fn); },
};

bus.on('message.received', ({ channelId, msg }) => messagesStore.append(channelId, msg));
```

Ownership: each store has **one file**, **one slice**, and **one owner doc**.
See [02-stores.md](02-stores.md).

### Channel (`channel/channel.js`)

One instance per opened channel. Owns its view state and its subviews. The
`ChannelRegistry` keeps a pool; the currently visible one is `activate()`d.

```js
export class Channel {
  constructor(id, hostEl) {
    this.id = id;
    this.hostEl = hostEl;
    this.unsubs = [];
    this.viewState = {
      filesPath: null, filesView: 'source',
      terminalBuffer: [], terminalCwd: null,
      scrollAnchor: null, draftText: '',
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

  activate() {
    for (const v of Object.values(this.views)) v.activate();
    this.unsubs.push(messagesStore.subscribe(this._onMessages));
    const conn = transport.e2ee.forChannel(this.id);
    conn?.getMessages(this.id);
    conn?.getActivity(this.id);
  }

  deactivate() {
    for (const v of Object.values(this.views)) v.deactivate();
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
  }

  destroy() {
    this.deactivate();
    this.viewState = null;  // release memory
  }
}
```

See [03-channel-lifecycle.md](03-channel-lifecycle.md).

### View (`channel/views/<name>-view.js`)

Owns one DOM region inside a channel. Mounts/unmounts cleanly. Subscribes
to stores on activate, unsubscribes on deactivate.

```js
export class ChatView {
  constructor(channel) {
    this.channel = channel;
    this.root = null;
    this.unsubs = [];
  }
  activate() {
    this.root = this.channel.hostEl.querySelector('[data-view="chat"]');
    this.render();
    this.unsubs.push(messagesStore.subscribe(this._onChange));
  }
  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    this.root = null;
  }
  _onChange = ({ channelId }) => {
    if (channelId !== this.channel.id) return;
    this.render();
  };
  render() { /* ... */ }
}
```

See [05-views.md](05-views.md).

### Shell (`shell/`)

Layout, routing, tab switching, rail, dropdowns, account menu. Never
imports from `channel/` or stores except through the router.

```js
// shell/router.js
export const router = {
  navigate(tab, channelId) {
    location.hash = `${tab}/${channelId}`;
    uiStore.setTab(tab);
    channelRegistry.activate(channelId);
  },
};
```

See [06-shell.md](06-shell.md).

## Directory layout

```
frontend/src/dashboard/
├── main.js               # bootstrap; wires stores, transport, shell, registry
├── core/
│   ├── bus.js            # typed pub/sub
│   ├── store.js          # store base (subscribe helper)
│   └── log.js            # scoped logger
├── transport/
│   ├── e2ee-dispatcher.js  # wire events → bus events
│   ├── sse.js              # Skrift notifications bridge
│   ├── rest.js             # /api/devices/, etc.
│   └── vendor/
│       └── e2ee.js         # (copy-ported from v1 vendor/)
├── domain/
│   ├── devices-store.js
│   ├── channels-store.js
│   ├── messages-store.js
│   ├── activity-store.js
│   ├── files-store.js
│   ├── terminal-store.js
│   ├── tasks-store.js
│   ├── complications-store.js
│   ├── presence-store.js   # agent active, plan mode, harness
│   └── ui-store.js         # tab, overlay, rail
├── channel/
│   ├── channel.js          # Channel class
│   ├── registry.js         # ChannelRegistry
│   └── views/
│       ├── chat-view.js
│       ├── files-view.js
│       ├── terminal-view.js
│       ├── tasks-view.js
│       ├── console-view.js
│       ├── complications-view.js
│       └── browser-view.js
├── shell/
│   ├── app.js              # top-level mount
│   ├── router.js
│   ├── layout.js           # builds the shell DOM (no template HTML)
│   ├── tabs.js
│   ├── rail.js
│   ├── sidebar.js
│   ├── dropdown.js
│   └── channel-panel.js    # device/channel list
├── util/
│   ├── html.js             # (port from v1)
│   ├── time.js             # (port from v1)
│   ├── format.js           # (port from v1)
│   └── markdown.js         # (port from v1 vendor/)
└── styles/
    ├── index.css
    ├── tokens.css
    ├── base.css
    ├── layout.css
    ├── shell.css
    ├── chat.css
    ├── files.css
    ├── terminal.css
    ├── console.css
    └── rail.css
```

## What is *not* in v2

- **No legacy.js.** `main.js` imports exactly what it needs.
- **No `window.*` bridges.** Playwright smoke tests use `import()` or a
  dedicated test harness module.
- **No 380-line HTML template.** `dashboard.html` is ~30 lines: meta,
  stylesheet link, `<div id="app"></div>`, script tag. The shell builds the
  DOM in JS from the `layout.js` function.
- **No per-channel state in global Maps.** `filesPath`, `terminalBuffer`,
  etc. live on `Channel.viewState`.
- **No `setInterval(renderChannelPanel, 15000)`.** The panel re-renders on
  subscription.
