# 05 — Views

A view owns one DOM region. Views are per-channel (mounted by `Channel`) or
shell-level (mounted once by `shell/app.js`). The lifecycle is the same.

## Contract

Every view implements:

```js
class SomeView {
  constructor(channel /* or null for shell views */) {
    this.channel = channel;
    this.root = null;
    this.unsubs = [];
  }

  activate() {
    this.root = this._resolveRoot();
    this._wireDom();
    this._subscribe();
    this.render();
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    this.root = null;
    // DOM cleanup: remove listeners added via the root element; GC'd with it.
  }

  render() { /* rebuild DOM subtree from state */ }

  _resolveRoot() { /* query inside this.channel.hostEl */ }
  _wireDom() { /* event handlers on this.root */ }
  _subscribe() { /* push store.subscribe() unsubscribers to this.unsubs */ }
}
```

Rules:

- **`activate()` and `deactivate()` must be idempotent.** Double-call is a
  no-op.
- **No work outside `activate()`/`deactivate()`.** The constructor creates
  an inert object.
- **No globals.** Anything that needs to be remembered across activations
  lives on `this.channel.viewState` (per-channel) or the appropriate
  domain store (per-domain).
- **Views dispatch intents, never call transport.** `bus.emit('intent.*', ...)`.
- **Views subscribe to stores, not to the bus.** The store is the view's
  contract with the domain. (Shell views that don't have a store — e.g.,
  dropdown — may subscribe to `uiStore`.)

## Per-channel views

Each `Channel` instantiates these on construction:

### `ChatView`
- **Slot**: the chat overlay body — message list + composer.
- **Reads**: `messagesStore.forChannel(id)`, `presenceStore`, `unreadStore`.
- **Writes**: `viewState.draftText`, `viewState.scrollAnchor`.
- **Intents**: `send_message`, `stop_agent`.

### `ConsoleView`
- **Slot**: the activity panel (tool use, reasoning).
- **Reads**: `activityStore.forChannel(id)`.
- **Writes**: `viewState.unreadHighlightLastSeen`, scroll anchor.
- **Owns the auto-scroll logic.** Scroll state is an instance field
  (`this._oldestAutoScrollTarget`), not a global. This is the single most
  important change from v1.

### `FilesView`
- **Slot**: the files tab (tree + viewer + mode switcher).
- **Reads**: `filesStore.forChannel(id)`.
- **Writes**: `viewState.filesPath`, `viewState.filesView`, `viewState.filesLineWrap`.
- **Intents**: `files_list`, `files_read`.
- Internally decomposes into `FileTreePanel`, `FileViewerPanel`,
  `FileModeBar`. Each is a child view with its own `activate`/`deactivate`,
  mounted by `FilesView`.

### `TerminalView`
- **Slot**: the terminal tab.
- **Reads**: `terminalStore.forChannel(id)`.
- **Writes**: `viewState.terminalCwd`, `viewState.terminalBuffer`,
  `viewState.terminalCmdHistory`, etc.
- **Intents**: `terminal_exec`.

### `TasksView`
- **Slot**: the tasks sidebar section.
- **Reads**: `tasksStore.forChannel(id)`.
- **Writes**: none (tasks are agent-authoritative for now).
- **Intents**: none yet. When CRUD is added: `task_create`, `task_toggle`, etc.

### `ComplicationsView`
- **Slot**: the complications strip above the viewer.
- **Reads**: `complicationsStore.forChannel(id)`.
- **Intents**: `resolve_complication`.

### `BrowserView`
- **Slot**: the `#tab-browser` region.
- **Reads**: `viewState.browserTabs` (per-channel).
- **Intents**: none beyond navigation.

## Shell views

Mounted once by `shell/app.js`. Alive for the session.

### `ChannelPanelView`
- **Slot**: left sidebar.
- **Reads**: `devicesStore`, `channelsStore`, `unreadStore`.
- **Intents**: emits `navigate(tab, channelId)` via the router.

### `RailView`
- **Slot**: bottom console rail.
- **Reads**: `uiStore`, `unreadStore` (aggregate).
- **Writes**: `uiStore.setRailState`.

### `SidebarSectionsView`
- **Slot**: sidebar sections (tasks, activity, devices).
- **Reads**: `uiStore` (collapse state), forwards to relevant views.

### `DropdownView`
- **Slot**: global dropdown menus.
- **Reads**: `uiStore.openDropdown`.

### `TopBarView`
- **Slot**: mobile nav, account menu.
- **Reads**: `uiStore`, `channelsStore` (for current name).

## Rendering strategy

Start imperative: each view's `render()` rebuilds its DOM subtree from state.
Simple, obvious, easy to debug. If a specific view becomes hot (console,
chat message list) and imperative re-render is slow, move **only that view**
to an incremental strategy (virtual list, diffing). Don't adopt a framework
wholesale.

Guidelines:

- `render()` should be pure from (state → DOM). No hidden state in the
  function.
- Event handlers attach inside `render()` (fresh handlers each render) OR
  via event delegation on the root (handlers set in `_wireDom()`, survive
  re-renders). Prefer delegation for lists.
- Use `escapeHtml` for any user-provided string going into `innerHTML`.
- Avoid inline styles; CSS lives in stylesheets. One stylesheet per view
  family is fine (`chat.css`, `files.css`, `terminal.css`).

## Shadow DOM?

No. CSS scoping via a data attribute on the view root (`data-view="chat"`)
is sufficient and easier to debug. Shadow DOM breaks too many conveniences
(devtools inspection, global theme variables, form autofill).

## Child views

Views can have child views. Composition is explicit: the parent instantiates
children in its constructor, activates/deactivates them in its own
`activate`/`deactivate`. `FilesView` → `FileTreePanel` + `FileViewerPanel`
is the canonical example.

## Testing views

Headless DOM (jsdom) or Playwright:

```js
test('ChatView renders messages from store', async () => {
  const channel = new Channel('ch1');
  const host = document.createElement('div');
  host.innerHTML = '<div data-view="chat"></div>';
  channel.hostEl = host;

  messagesStore.append('ch1', { id: 'm1', content: 'hello', sender: 'client' });
  channel.views.chat.activate();

  expect(host.querySelector('.msg .msg-text').textContent).toContain('hello');
});
```

Views should be testable without transport (stubbed stores) and without the
full shell (direct mount).
