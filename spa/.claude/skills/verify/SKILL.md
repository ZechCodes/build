---
name: verify
description: Build/launch/drive recipe for verifying spa/ changes end-to-end in a real browser
---

# Verifying spa/ changes

## Setup

`@build/secure-transport` is a file dep two levels above the repo root. Before
`npm install` in a fresh worktree:

```sh
ln -sfn ~/Projects/8ly/build-secure-transport "$(git rev-parse --show-toplevel)/../build-secure-transport"
cd spa && npm install
```

## Launching without the full E2EE stack

The real app needs the relay + bridge + passkey auth — not reachable headless.
For client-only changes, mount the real module under test from a scratch HTML
page in `spa/` (Vite serves any root-level .html in dev) with fake plumbing,
e.g. `mountTerminalPane(host, { attach, input, resize, onExit })` with an
`attach` that just calls `onSnapshot`, and `input` pushing into a
`window.__inputLog` array.

Vite's dep optimizer crawls `index.html` and dies on libsodium-wrappers' ESM
build (`Could not resolve './libsodium.mjs'` — the main config's shim doesn't
apply to the scan). Point the scanner at only your harness page:

```js
// spa/vite.verify.config.js (delete before committing)
import { defineConfig } from "vite";
export default defineConfig({
  optimizeDeps: { entries: ["your-harness.html"] },
  server: { port: 5199, strictPort: true },
});
```

`npx vite --config vite.verify.config.js`

A harness that imports the real `src/app.js` (to drive the shell — rail,
toolbar, feed — with a fake `App.call`) drags the scanner into the transport
anyway. Merge the main config (its libsodium shim serves the module in dev)
and keep the transport out of the scan; prismjs is CommonJS, so the optimizer
itself must stay on:

```js
import { mergeConfig } from "vite";
import base from "./vite.config.js";
export default mergeConfig(base, {
  base: "/",
  optimizeDeps: { entries: ["your-harness.html"], exclude: ["@build/secure-transport", "libsodium-wrappers", "libsodium"] },
  server: { port: 5199, strictPort: true },
});
```

Start the server as a background task, not with `&` — a shell-backgrounded
process dies with the tool call.

## Driving

Python Playwright is installed (pyenv 3.13.7, browsers cached):

```sh
uv run --python ~/.pyenv/versions/3.13.7/bin/python3.13 --with playwright==1.60.0 script.py
```

- Context: `new_context(viewport=..., has_touch=True, is_mobile=True)`.
- Trusted touch input: CDP `Input.dispatchTouchEvent` on
  `ctx.new_cdp_session(page)` (Playwright's touchscreen has no drag).
- `window.__buildTerminal` is the most recently mounted ghostty terminal
  (QA handle set by `pane.js`); `getViewportY()` reads scroll position in rows.
- Enter/leave the alternate screen with
  `window.__buildTerminal.write('\x1b[?1049h')` / `'\x1b[?1049l'`.

## Gotchas

- Each CDP round trip takes ~100-400ms headless, and ghostty renders
  synchronously per wheel event — you cannot produce a fast gesture with real
  clocks. For velocity/momentum paths, dispatch in-page `TouchEvent`s and pin
  `timeStamp` via `Object.defineProperty(event, "timeStamp", { value: ts })`.
- ghostty-web 0.4.0 has exactly one touch listener: canvas `touchend` →
  `preventDefault()` + textarea focus. Everything else (wheel, mouse) is on
  the host element `term.open()` received, capture-phase.
- Collect `page.on("console")` errors — ghostty logs real problems there
  (e.g. preventDefault on non-cancelable events).
