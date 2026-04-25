# Dashboard smoke harness

Playwright-driven smoke test for the dashboard bundle. Logs in via the dummy
auth provider, asserts the rewritten dashboard loads at `/dashboard/`, verifies
canonical assets, and injects synthetic store events through `window.__v2debug`.

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

Exit code 0 means every check passed.

## What it covers

- Bundle assets serve with content-hash query strings.
- Canonical `dashboard.css` and `dashboard.js` assets are present.
- Core rewritten dashboard DOM exists (`.v2-app`, sidebar, viewer, rail,
  chat overlay, complications, dropdown layer).
- `window.__v2debug` exposes the bus, router, stores, registry, and E2EE pool.
- Synthetic device/channel store events render a channel and clicking it updates
  the active channel.

## What it does NOT cover

- Crypto correctness (sealed-box / secretbox round-trip).
- Real network round-trip with a live device daemon.
- Visual regressions (screenshot diffing).

## Adding new assertions

Keep this harness focused on page boot and canonical route/bundle regressions.
Feature-specific Playwright checks live in the sibling `verify-*.mjs` scripts.
