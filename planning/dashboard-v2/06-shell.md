# 06 — Shell

The shell is the app chrome: layout, routing, tabs, rail, dropdowns. It
knows about `uiStore` and the router. It does not know about messages,
files, terminal, or any per-channel concern.

## Responsibilities

- Build the top-level DOM scaffold (sidebar, viewer, rail, overlay hosts).
- Own the router — hash ↔ state ↔ registry activation.
- Host shell views (channel panel, rail, sidebar sections, dropdown, top bar).
- Expose a `channelHost` DOM element where the active `Channel` mounts its
  views.

## Bootstrap sequence

```js
// main.js
import './styles/index.css';
import { initStores } from './domain/index.js';
import { initTransport } from './transport/index.js';
import { initShell } from './shell/app.js';

initStores();         // Create stores, subscribe to bus.
initTransport();      // Connect SSE, wire intent dispatcher, init E2EE pool.
initShell();          // Mount shell DOM, start router.
```

`initShell()` does, in order:

1. Build the layout DOM into `document.getElementById('app')`.
2. Mount shell views (channel panel, rail, etc.).
3. Initialize the router: parse `location.hash`, call
   `router.navigate(tab, channelId)` if present.
4. Listen for `hashchange`.

## Layout construction

The v1 template has 380 lines of static HTML. v2's template has ~30. The
shell builds the layout in JS:

```js
// shell/layout.js
export function buildLayout(root) {
  root.innerHTML = `
    <div class="app content-area">
      <div class="channel-panel" id="channel-panel"></div>
      <div class="viewer-panel">
        <div class="mobile-nav" id="mobile-nav"></div>
        <div class="viewer-body">
          <div class="tab-panels" id="tab-panels">
            <div class="tab-panel" data-tab="files" id="tab-files"></div>
            <div class="tab-panel" data-tab="browser" id="tab-browser"></div>
          </div>
        </div>
        <div class="bottom-rail" id="bottom-rail"></div>
      </div>
      <div class="chat-overlay" id="chat-overlay"></div>
    </div>
  `;
}
```

Each channel mounts its views into these slots:

- `ChatView` → `#chat-overlay`
- `FilesView` → `#tab-files`
- `TerminalView` → `#tab-terminal` (dynamically added or static, TBD)
- `TasksView` → sidebar tasks section
- `ConsoleView` → `#bottom-rail` activity panel
- `ComplicationsView` → above `#tab-files`
- `BrowserView` → `#tab-browser`

The `Channel.hostEl` is set to the app root; each view queries its own
region via `data-view` or id selector.

## Router

```js
// shell/router.js
import { channelRegistry } from '../channel/registry.js';
import { uiStore } from '../domain/ui-store.js';

class Router {
  constructor() {
    this.history = [];       // channelId history stack
    this.index = -1;
  }

  init() {
    window.addEventListener('hashchange', () => this._onHash());
    this._onHash();
  }

  navigate(tab, channelId) {
    const hash = channelId ? `${tab}/${channelId}` : tab;
    if (location.hash !== `#${hash}`) location.hash = hash;
    uiStore.setTab(tab);
    if (channelId) {
      channelRegistry.activate(channelId);
      this._pushHistory(channelId);
    } else {
      channelRegistry.deactivateAll();
    }
  }

  back() { /* move index, activate */ }
  forward() { /* move index, activate */ }

  _onHash() {
    const [tab, channelId] = location.hash.replace(/^#/, '').split('/');
    if (!tab) return;
    this.navigate(tab, channelId);
  }

  _pushHistory(channelId) {
    if (this.history[this.index] === channelId) return;
    this.history = this.history.slice(0, this.index + 1);
    this.history.push(channelId);
    this.index = this.history.length - 1;
  }
}

export const router = new Router();
```

The router is the **only** owner of channel history. In v1 this leaked into
`state.channelHistory` + `_navigatingHistory` + `channels/select.js`. In v2
it's here.

## Tabs

Tabs are just a `uiStore.tab` value plus CSS that shows the matching
`[data-tab="xyz"]` panel. No per-tab subscription; the shell does a single
`body.dataset.tab = tab` update and stylesheets do the rest.

## Rail

`RailView` owns the bottom console. States: `collapsed`, `open`, `expanded`.
State lives in `uiStore.railState`. The unread badge reads
`unreadStore.aggregate()` (a computed selector).

## Dropdowns

One active dropdown at a time. `uiStore.openDropdown` holds the id.
`DropdownView` subscribes, shows/hides. Click-outside handler on the shell
root closes.

## Chat overlay

v1 physically relocates `#tab-chat` DOM into the overlay on boot
(`app/init.js` → `relocateChatLayout`). v2: the overlay is a layout slot
from the start. `ChatView` mounts directly into `#chat-overlay`. No
relocation.

## Electron-only keybindings

Kept at shell level (`shell/keybindings.js`), detects `window.buildElectron`
on init. Emits navigation intents via the router, never touches state.

## What the template contributes

The template (`dashboard_v2.html`) contributes:

- `<meta>`, `<title>`, font/stylesheet links.
- `<div id="app"></div>`.
- `<script src="..."></script>` for the bundle.
- The user name and CSP nonce passed as data attributes on `<body>` if the
  shell needs them.

That's it. Everything else is JS.
