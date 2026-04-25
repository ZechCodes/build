# 07 — Migration & Delivery Plan

The dashboard rewrite has been promoted to `/dashboard/`. The previous
dashboard source, template, and bundle entries have been removed from the
active codebase.

## Current setup

- **Route**: `/dashboard/`.
- **Template**: `themes/build/templates/dashboard.html` (minimal shell).
- **Python controller**: `build_app/dashboard.py` registered in
  `app.yaml`, `app.development.yaml`, `app.compose.yaml`.
- **Source tree**: `frontend/src/dashboard/`.
- **Bundle**: `themes/build/static/dist/js/dashboard.js` +
  `dist/css/dashboard.css`, built by the dashboard esbuild entries.
- **API**: unchanged. The dashboard uses the same `/api/devices/`, WebSocket
  relay, and Skrift SSE as the previous implementation.

## What shares code with v1

**Nothing on the frontend.** The rewritten `util/`, markdown renderer, transport,
stores, and views are now the dashboard implementation. The backend API surface
is unchanged.

## Waves

Each wave is shippable: v2 can be opened at `/dashboard/` and exercises
the subset of functionality delivered to date. Don't move to the next wave
until the current one is smoke-tested.

### Wave 0 — Scaffold (done during planning)

- Create `/dashboard/` route, template, empty bundle.
- "Dashboard — scaffolded" placeholder renders.
- `planning/dashboard/` docs committed.
- No domain code. No stores. No transport wiring.

**Exit criteria**: visiting `/dashboard/` as a logged-in user shows the
placeholder page; the v2 JS bundle loads without errors.

### Wave 1 — Core + stores

Deliver: `core/bus.js`, `core/store.js`, `core/log.js`.

Create empty stores: `devicesStore`, `channelsStore`, `messagesStore`,
`activityStore`, `presenceStore`, `unreadStore`, `filesStore`,
`terminalStore`, `tasksStore`, `complicationsStore`, `uiStore`.

Each store has the contract from [02-stores.md](02-stores.md) but no bus
bindings yet.

Unit tests for the bus and for two representative stores (messages,
unread).

**Exit criteria**: tests pass; importing any store does nothing visible.

### Wave 2 — Transport + dispatcher

Port `vendor/e2ee.js` into `transport/vendor/e2ee.js`. Implement
`transport/e2ee-dispatcher.js`, `transport/e2ee-pool.js`, `transport/sse.js`,
`transport/rest.js`, `transport/coordinator.js`, `transport/intent-dispatcher.js`.

Wire store bindings to bus events per [04-transport.md](04-transport.md).

At this point, opening `/dashboard/` and logging in should populate
stores from live devices. Shell still renders the placeholder (no views
yet) so verify via `console.log` and a `debug()` helper that dumps store
contents.

**Exit criteria**: live E2EE frames from a device populate `messagesStore`,
`channelsStore`, `activityStore` correctly. Offline/online transitions
reflected in `devicesStore`.

### Wave 3 — Shell + channel panel

Deliver: `shell/app.js`, `shell/layout.js`, `shell/router.js`,
`shell/channel-panel.js`, `shell/rail.js` (minimal), `shell/dropdown.js`,
`shell/sidebar.js`, basic `shell/tabs.js`.

v2 now shows the sidebar with devices and channels, reactive to
`devicesStore` + `channelsStore`. Clicking a channel navigates
(`router.navigate('files', channelId)`) but the viewer panel is empty.

**Exit criteria**: channel list renders, unread badges update on incoming
messages, device dots turn green/yellow/gray correctly. Hash routing
works; reload preserves selection.

### Wave 4 — Channel controller + ChatView + ConsoleView

Deliver: `channel/channel.js`, `channel/registry.js`, `channel/views/chat-view.js`,
`channel/views/console-view.js`.

v2 now has working chat: select a channel, see messages, see agent
activity, send a message, get responses. The two most-used views live.

**Exit criteria**: ChatView parity with v1 (messages, composer, suggested
actions, interaction cards, stop button). ConsoleView parity (tool use,
reasoning, auto-scroll that doesn't drift between channels).

### Wave 5 — FilesView + TerminalView

Deliver: `channel/views/files-view.js` (+ child views: tree, viewer, mode
bar, diff), `channel/views/terminal-view.js`.

Port `files/syntax.js`, `files/diff.js`, `files/html-preview.js` from v1
with minimal changes.

**Exit criteria**: files tab fully functional (tree, viewer, diff,
markdown, HTML preview). Terminal fully functional (command history,
completions, cwd tracking).

### Wave 6 — TasksView + ComplicationsView + BrowserView + polish

Deliver remaining views. Review-flow UI (no longer a toast stub).

**Exit criteria**: v2 feature parity with v1. No toast stubs.

### Wave 7 — Cutover

Completed: `/dashboard/` serves the rewritten dashboard directly, the parallel
route was removed, v1 source/template files were deleted, and esbuild emits only
canonical dashboard bundles.

**Exit criteria**: no active app or test references to the old parallel
dashboard route, controller, source path, or bundle names.

## Rollback

If the rewritten dashboard has a showstopper after cutover, revert the cutover
commit to restore the previous dashboard source, template, route registration,
and bundle entries from git history.

## Risk list

- **E2EE protocol coupling**: v1's wire format is stable, but if a device
  update adds an event type, dispatcher must handle it. Mitigation: log
  unknown event types loudly in the dispatcher.
- **Scroll parity**: ConsoleView auto-scroll is the most-complex port.
  Budget extra time in Wave 4; test with long sessions.
- **Markdown renderer drift**: `vendor/markdown.js` is custom, not
  spec-compliant. Port it byte-for-byte unless there's a known bug.
- **Style regressions**: v2 stylesheets are a fresh write. Budget a style
  pass per wave.

## Out of scope for this migration

- Backend rewrite.
- Transport protocol changes.
- Accessibility audit (separate initiative; flag issues but don't block
  waves).
- Offline mode with IndexedDB persistence (post-cutover optional
  enhancement).
- Switching to a framework (React, Svelte, etc.). Keep it vanilla for now
  — add one later if signals call for it.
