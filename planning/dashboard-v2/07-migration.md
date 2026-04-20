# 07 — Migration & Delivery Plan

v2 is built in parallel with v1 at `/dashboard-v2/`. When v2 reaches
parity and passes smoke tests, the route is promoted to `/dashboard/` and
v1 is deleted. There is no gradual in-place migration of v1 modules — the
v1 architecture is the problem, so we leave it alone and build fresh.

## Parallel-route setup

- **Route**: `/dashboard-v2/` (v1 stays at `/dashboard/`).
- **Template**: `themes/build/templates/dashboard_v2.html` (minimal shell).
- **Python controller**: `build_app/dashboard_v2.py` registered in
  `app.yaml`, `app.development.yaml`, `app.compose.yaml`.
- **Source tree**: `frontend/src/dashboard-v2/` (no shared code with v1).
- **Bundle**: `themes/build/static/dist/js/dashboard-v2.js` +
  `dist/css/dashboard-v2.css`, built by a second pair of esbuild entry
  points in `esbuild.config.mjs`.
- **API**: unchanged. v2 uses the same `/api/devices/`, WebSocket relay,
  and Skrift SSE as v1.

Switching between them during development: just change the URL.

## What shares code with v1

**Nothing on the frontend.** v2's `util/`, `vendor/markdown.js`, etc. are
ports (copy-modify-test), not shared imports. This is deliberate — shared
modules would re-couple v1 and v2 and make v2 inherit v1's drift.

The backend is unchanged. v1 and v2 hit the same routes.

## Waves

Each wave is shippable: v2 can be opened at `/dashboard-v2/` and exercises
the subset of functionality delivered to date. Don't move to the next wave
until the current one is smoke-tested.

### Wave 0 — Scaffold (done during planning)

- Create `/dashboard-v2/` route, template, empty bundle.
- "Dashboard v2 — scaffolded" placeholder renders.
- `planning/dashboard-v2/` docs committed.
- No domain code. No stores. No transport wiring.

**Exit criteria**: visiting `/dashboard-v2/` as a logged-in user shows the
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

At this point, opening `/dashboard-v2/` and logging in should populate
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

Redirect `/dashboard/` → `/dashboard-v2/`. Delete v1 source
(`frontend/src/dashboard/`, `themes/build/templates/dashboard.html`). Move
v2 to `/dashboard/`; delete `/dashboard-v2/` route.

Delete v1 esbuild entries. Rename `dashboard-v2` → `dashboard` in source
paths. Update `planning/` docs to reflect that v2 is now the only
dashboard.

**Exit criteria**: `grep -r "dashboard-v2"` returns nothing.

## Parallel maintenance

During the rewrite, v1 keeps getting bug fixes as needed — but only for
shipping blockers. No new features in v1. Any planned feature goes to v2.

If v1 diverges significantly during the rewrite (e.g., a new channel
property is added), note it in `07-migration.md` so the v2 ports stay in
sync.

## Rollback

If v2 has a showstopper after cutover: flip the `/dashboard/` controller
back to the v1 template/bundle (both still sit in git history one commit
prior). The frontend/backend decoupling means rollback is a single commit
revert.

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
