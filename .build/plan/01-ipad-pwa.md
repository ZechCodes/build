# Stage 01 — Make the web client a correct installable iPad PWA

## Goal

Configure the Build v2 web client (`spa/`) so it installs to the iPad home
screen via Safari "Add to Home Screen" and runs correctly in **standalone**
mode: full-screen, respecting the iPad's safe areas (status bar, home
indicator, rounded corners), with a launch/status-bar chrome that matches the
app's light/dark theme, and with the login redirect kept *inside* the installed
app instead of bouncing the user out to Safari.

The app already ships most of the scaffolding (a web manifest, Apple meta tags,
correctly sized icons, and a deliberately cache-free push service worker). This
stage closes the gaps that make standalone iPad behave wrong, and does **not**
add offline caching (see Assumptions).

## Context a cold agent needs

- **Which app.** The web client is `spa/` (`package.json` name `build-spa`) — a
  no-framework, Vite-bundled, vanilla-ESM SPA. It is **not** `web/` (QA scripts)
  or `skriftapp/` (the Python host).
- **How it is served.** `spa` builds into `skriftapp/buildapp/static/`. The
  Python host (`skriftapp/buildapp/controllers.py`, `BuildController`, path
  `/app`) serves:
  - `/app/` → `static/index.html` with `{{USER0}}` substituted (redirects to
    `/auth/login?next=/app/` when there is no Skrift session).
  - `/app/sw.js` → `static/sw.js` with `Cache-Control: no-cache`.
  - `/app/static/{path}` → `static/{path}`; anything under `static/assets/` is
    served `immutable`. The manifest and PNG icons are served from
    `/app/static/…` because Vite copies `spa/public/*` to the bundle root.
- **Auth is vendored.** `/auth/login` belongs to the third-party Skrift package
  (`skrift[passkeys]==0.2.0a10`, vendored wheel). **Do not** try to edit the
  login page; it is outside this repo. The only lever here is the manifest
  `scope`.
- **The service worker is cache-free on purpose.** `spa/public/sw.js` has **no**
  `fetch` handler. The E2EE design rule is that the app must never be served
  from a stale cache. **Do not add a `fetch`/cache handler.** It only handles
  `push` / `notificationclick`.
- **Theme.** `spa/src/styles.css`: `:root` is light (`--bg:#fafafa`),
  `:root[data-theme="dark"]` is dark (`--bg:#15171c`). `spa/public/theme-boot.js`
  stamps `data-theme` before first paint from stored preference / OS.
- **Test convention.** Frontend tests are Vitest (`spa/test/*.test.js`, run with
  `npm test`). Static-asset tests read the real file with
  `readFileSync(fileURLToPath(new URL("../public/…", import.meta.url)))` and
  assert on its contents — see `spa/test/sw.test.js` for the exact pattern.
  Follow TDD: write the failing assertion first, then edit the asset.

## Current state (verified) and the gaps to close

| Area | Current | Gap for iPad standalone |
|------|---------|-------------------------|
| Manifest | `display:standalone`, `scope:/app/`, `start_url:/app/`, icons 192+512 | `scope` excludes `/auth/*`; missing `id`, `lang`, `orientation`, `display_override`; `theme_color` (`#5b62e8`) doesn't match the app bg |
| Viewport | `width=device-width, initial-scale=1, interactive-widget=resizes-content` | no `viewport-fit=cover` → `env(safe-area-inset-*)` is **0** in standalone, so existing safe-area CSS is inert and content can sit under the home indicator |
| Apple meta | `apple-mobile-web-app-capable=yes`, `status-bar-style=default`, `apple-mobile-web-app-title=Build`, `apple-touch-icon` (180×180) | fine; keep. `theme-color` meta is light-only (`#fafafa`) — add a dark-mode variant |
| Icons | 180 / 192 / 512 PNGs present and correctly sized | ok; no change |
| Service worker | registered at `/app/` scope, cache-free push worker | ok; no change (do **not** add caching) |
| Standalone CSS | some `safe-area` padding on the terminal key bar only | shell (sidebar, main, sheets, FAB) does not pad for safe areas; no rubber-band/overscroll control for the installed app |

## What to build

All changes are in `spa/`. There is **no server change** — broadening the
manifest `scope` is a content-only edit, and every asset already has a route.

### 1. Viewport — opt into safe areas (`spa/index.html`)

Add `viewport-fit=cover` to the existing viewport meta so
`env(safe-area-inset-*)` returns real values in standalone. Keep
`interactive-widget=resizes-content`:

```html
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover, interactive-widget=resizes-content" />
```

### 2. Theme-color that matches the app (`spa/index.html`)

Replace the single light-only `theme-color` meta with a light + dark pair so the
iPad standalone title/status chrome matches the actual background in both
themes:

```html
<meta name="theme-color" content="#fafafa" media="(prefers-color-scheme: light)" />
<meta name="theme-color" content="#15171c" media="(prefers-color-scheme: dark)" />
```

Keep `apple-mobile-web-app-status-bar-style=default` (see Assumptions). Leave the
other apple-* meta tags and the `apple-touch-icon` link as they are.

### 3. Manifest (`spa/public/manifest.webmanifest`)

- Change `scope` from `/app/` to `/` so the `/auth/login` redirect stays inside
  the installed app (iPad opens out-of-scope navigations in Safari and orphans
  the session). Keep `start_url` at `/app/`.
- Set `theme_color` and `background_color` to `#fafafa` (the light `--bg`), so
  the OS launch background matches the app's first paint. (`background_color`
  is already `#fafafa`.)
- Add: `"id": "/app/"`, `"lang": "en"`, `"dir": "ltr"`,
  `"orientation": "any"` (an iPad is used both ways),
  `"display_override": ["standalone"]`.
- Leave `name`, `short_name`, `description`, and the icon list unchanged.

Note: the manifest `scope` (`/`) is independent of the service-worker
registration scope (`/app/` in `spa/src/push.js`) — do not change the SW scope.

### 4. Standalone shell CSS (`spa/src/styles.css`)

Make the app shell respect safe areas and behave like an installed app, without
regressing the desktop/browser layout (all rules are inset-based, and
`env(safe-area-inset-*, 0px)` is 0 everywhere except iPad standalone):

- On `html, body`: add `overscroll-behavior: none` (kill the rubber-band bounce
  that reveals the page edge in standalone). `-webkit-text-size-adjust:100%` is
  already set on `body`; keep it.
- Pad the fixed shell for safe areas so nothing hides under the status bar,
  home indicator, or rounded corners. The shell is `#shell` / `#sidebar` /
  `main#root`. Apply `env(safe-area-inset-*)`:
  - the sidebar's left/top/bottom insets,
  - the main surface's right/bottom insets,
  - any bottom-anchored floating UI (`#fab`, the `.actionbar`, the sheet
    `#sheet`) so it clears the home indicator.
  Prefer `padding` (or `margin`) added to existing values, e.g.
  `padding-bottom: calc(<existing> + env(safe-area-inset-bottom, 0px));`.
  The terminal key bar already does this (styles.css ~L886) — match that idiom.
- Guard against long-press selection / tap-flash on chrome that is buttons, not
  text: the icon buttons and nav rows should keep
  `-webkit-tap-highlight-color: transparent` (already used in places) — extend
  to the sidebar nav rows if not covered.

Do **not** change the `100vh/100dvh` full-height rules — they already handle the
on-screen-keyboard resize and are correct.

### 5. Tests (`spa/test/pwa.test.js`, new)

Write these **first** (red), then make the edits above (green). Read the real
files with the `readFileSync(fileURLToPath(new URL(...)))` pattern from
`spa/test/sw.test.js`. Assert:

- **index.html**
  - viewport meta contains `viewport-fit=cover` (and still has
    `width=device-width` and `interactive-widget=resizes-content`).
  - `apple-mobile-web-app-capable` is `yes`.
  - there is a `theme-color` meta for `(prefers-color-scheme: dark)` and one for
    light (or a default), and the dark one is the dark `--bg` `#15171c`.
  - the `manifest` link and `apple-touch-icon` link are still present.
- **manifest.webmanifest** (parse as JSON)
  - `display` is `standalone`.
  - `scope` is `/` and `start_url` is `/app/`.
  - `id`, `lang`, `orientation` are present; `orientation` is `any`.
  - `theme_color` and `background_color` are both `#fafafa`.
  - icons include 192×192 and 512×512 entries.
- **styles.css** (string contains, matching the sw.test.js file-read style)
  - contains `env(safe-area-inset-bottom` at least once outside the existing
    key-bar rule (assert a count ≥ 2, since the key bar already has one), and
    contains `overscroll-behavior`.

Keep the existing `spa/test/sw.test.js` and `spa/test/push.test.js` green
(no behavior change to the worker or push).

## Verification

```bash
cd spa
npm install        # if node_modules is absent
npm test           # new pwa.test.js green; sw/push tests still green
npm run build      # emits skriftapp/buildapp/static/ with the updated manifest + index
```

Manual/device confirmation (report, don't block on it): open the built `/app/`
in Safari on an iPad (or the iPad simulator / responsive design mode), "Add to
Home Screen", launch from the home icon, and confirm: it opens full-screen with
no Safari chrome; content clears the status bar and home indicator; light/dark
status chrome matches the app; and hitting the app while logged out lands on the
login page *inside* the standalone window (not Safari) and returns to `/app/`
after login.

## Definition of done

- `npm test` passes, including the new `pwa.test.js`; `npm run build` succeeds.
- Manifest is `standalone` with `scope:/`, `start_url:/app/`, matching
  `theme_color`/`background_color`, and the added identity/orientation fields.
- `index.html` opts into safe areas (`viewport-fit=cover`) and has theme-aware
  `theme-color`.
- The shell pads for iPad safe areas and suppresses standalone overscroll,
  with no change to desktop/browser layout.
- The service worker remains cache-free (no `fetch` handler added).
- Commit the change (project rule: commit as you go; run semgrep + gitleaks
  before committing; security checklist 100/100). No files touched outside
  `spa/` (and `skriftapp/buildapp/static/` is build output, gitignored).

## Assumptions

Recorded per the planning protocol; each was chosen as the most reasonable
default and will be revised if the reviewer says otherwise (see thread
message-5).

1. **Status bar style stays `default`** (content below the iOS status bar) rather
   than `black-translucent` (edge-to-edge under the status bar). `default` is the
   lower-risk correct baseline; going translucent would require top
   safe-area-inset padding on every top surface. If the reviewer wants the
   immersive look, add `status-bar-style=black-translucent` and a
   `padding-top: env(safe-area-inset-top)` on the top of the sidebar/main.
2. **Manifest `scope` is broadened to `/`** so the vendored-Skrift `/auth/login`
   redirect stays inside the installed app. The only same-origin routes are
   `/app/*` and `/auth/*` (the CMS root redirects to `/app/`), so scope `/` has
   no unwanted side effects.
3. **No launch splash screens.** `apple-touch-startup-image` sets are deferred —
   cosmetic, and they require generating per-device PNGs. Install icon and app
   boot work without them.
4. **No offline caching.** The service worker stays cache-free per the E2EE
   "never serve stale" rule; launching offline shows the browser's offline page.
   This is a deliberate design constraint, not an oversight.
