# Build SPA (v2 web client)

The task board / plan / diff / terminal web client. Vanilla ES modules, no
framework, bundled with Vite. Every dependency is self-hosted (libsodium, the
`@build/secure-transport` binding, ghostty-web with inlined wasm, Inter fonts)
— zero CDN at runtime.

## Layout

```
src/core/       runtime-agnostic logic, fully unit-tested:
  session.js      E2EE relay session (authenticate → device_key → session_init)
  diff.js         unified-diff parsing + review noise filter
  markdown.js     safe markdown rendering for plans/notifications
  notes.js        batched plan/diff comments → agent notes
  router.js       hash routes (#/board, #/task/<id>/<tab>, …)
src/views/      board, notifications, settings, task (plan+diff), gate
src/sheets/     modal sheets: new task, repo browser/new/clone/remote, add device
src/terminal/   the PTY drawer (ghostty-web) + its reconnecting E2EE session
src/            app shell: state+render dispatch, connection lifecycle,
                device store/picker, api client, comment popover
```

## Prereqs

`@build/secure-transport` resolves as `file:../../build-secure-transport/js` —
a sibling checkout of <https://github.com/ZechCodes/build-secure-transport>
next to this repo (same convention as `web/` and CI).

## Commands

```bash
npm install
npm run lint       # eslint: one rule — no function over complexity 10
npm test           # vitest: session core, diff, markdown, notes, router, terminal
npm run test:browser # Chromium layout regressions, also included in npm test
npm run build      # emits skriftapp/buildapp/static/ (served by BuildController)
npm run dev        # Vite dev server (proxy /api to a running skriftapp yourself)
```

Browser tests use the `playwright-core` dev dependency and require Chromium or
Chrome on `PATH`, or `CHROMIUM_PATH` set to its executable. The app image only
runs `npm run build`; it does not need a browser.

## Configuration

- `VITE_RELAY_URL` — the relay's WebSocket origin for `/ws/client`.
  Default `ws://localhost:18090` (local dev relay); production builds use
  `VITE_RELAY_URL=wss://relay.getbuild.ing npm run build`.

## Serving

`BuildController` serves `static/index.html` at `/app/` (substituting the
`{{USER0}}` avatar initial) and hashed assets at `/app/static/assets/*` with
immutable caching. The build output is gitignored — build it as part of the
image/deploy pipeline.

CSP needs: `script-src 'self'`, `style-src 'self' 'unsafe-inline'` (inline
style attributes), `connect-src 'self' data: <relay ws(s) origin>` (`data:`
because ghostty-web instantiates its wasm from a data: URL), `font-src 'self'`.
