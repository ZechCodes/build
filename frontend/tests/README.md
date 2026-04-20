# Dashboard smoke harness

Playwright-driven smoke test for the dashboard bundle. Logs in via the dummy
auth provider, asserts the page loads cleanly, then injects a fake `BuildE2EE`
instance and fires every event in `bindE2EEEvents` to catch the class of
regressions that only surface with a connected device.

## One-time setup

```bash
cd build-web/frontend/tests
npm install
npm run smoke:install   # downloads chromium into ~/Library/Caches/ms-playwright
```

## Running

Start a local dev server in another terminal:

```bash
cd build-web
SKRIFT_ENV=development uv run skrift serve --host 127.0.0.1 --port 8092
```

Then run the harness:

```bash
cd build-web/frontend/tests
BASE_URL=http://127.0.0.1:8092 npm run smoke
```

Exit code 0 means every check passed. The script also writes
`dashboard.png` (a screenshot of the dashboard's initial render) for visual
sanity checking.

## What it covers

- Bundle assets serve with content-hash query strings.
- Core DOM elements present (`#channel-panel-list`, `#file-content-body`,
  `#chat-overlay`, etc.).
- All `window.*` bridge functions exposed.
- `renderMarkdown` end-to-end.
- Every `switchTab(...)` path.
- `toggleChatOverlay`, `setConsoleState`, `addPendingFiles`.
- A fake `BuildE2EE` instance is registered into `state.e2eeConnections`
  and `bindE2EEEvents` is invoked on it. The harness then dispatches
  every event the real bridge listens for (`channel_list`, `messages`,
  `agent_event`, `terminal_output`, `file_read_result`,
  `file_diff_result`, `complication_update`, etc.) with realistic
  payloads, and asserts no `pageerror` was raised.

## What it does NOT cover

- Crypto correctness (sealed-box / secretbox round-trip).
- Real network round-trip with a live device daemon.
- Visual regressions (screenshot diffing).

## Adding new assertions

The synthetic event table lives near the bottom of `dashboard.smoke.mjs`.
Each row is `{ name, before?, detail, after? }`:

- `name` — E2EE event name fired on the fake instance.
- `before` (optional) — JS string `eval`d before dispatch to set state
  preconditions.
- `detail` — the `CustomEvent.detail` payload.
- `after` (optional) — JS string `eval`d after dispatch; truthy = pass.

Every row also gets a free "no pageerror" assertion.
