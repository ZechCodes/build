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
src/views/      board, workspace (Changes/Files/Tasks), notifications, settings, task, gate
src/sheets/     modal sheets: new task, repo browser/new/clone/remote, add device
src/terminal/   the PTY drawer (ghostty-web) + its reconnecting E2EE session
src/            app shell: state+render dispatch, connection lifecycle,
                device store/picker, api client, comment popover
```

Workspace Changes retains each directory's pane between tab selections. Its
visibility lifecycle suspends owned floating editors, dialogs, focus and visual
work; drafts, review marks, scroll and cache readers stay with the directory.
Hidden read/mutation results settle in their own records, and a deferred
checkout invalidation refreshes when that directory is shown.

## Prereqs

Use Node 22.23.4 (`.nvmrc`) and npm 12.0.2 (`packageManager`).
`@build/secure-transport` resolves as `file:../transport/js` inside this checkout.
Build maintains that package here; [transport ownership and tests](../transport/README.md)
and [import provenance](../transport/UPSTREAM.md) describe the boundary.
Install that package's own locked dependencies first, from the repo root:

```bash
nice -n 10 npm ci --prefix transport/js --ignore-scripts
```

SPA's `package-lock.json` is committed and portable. Use `npm ci
--legacy-peer-deps` for a fresh install. When editing dependencies, run
`npm install --legacy-peer-deps` and commit the updated lock with the manifest.
`.npmrc` preserves the peer-resolution policy and disables install scripts.
New registry resolutions must be at least seven days old; locked installs retain
the committed dependency versions.

If a reviewed fix must use a release younger than seven days, make a one-command
exception for that exact package (replace `package-name` and `x.y.z`):

```bash
nice -n 10 npm install package-name@x.y.z --save-exact --legacy-peer-deps --ignore-scripts --min-release-age-exclude=package-name
```

The package-name exception leaves the seven-day rule active for every other
package, including its transitive dependencies, and keeps lifecycle scripts
disabled. Record the reason and exact version with the manifest/lock review;
do not add the exception to `.npmrc` or use a wildcard. A younger transitive
fix needs its own explicit package exception. Ordinary installs use the commands
above again.


## Commands

```bash
nice -n 10 npm ci --legacy-peer-deps
nice -n 10 npm run lint       # eslint: one rule — no function over complexity 10
nice -n 10 npm test           # vitest: session core, diff, markdown, notes, router, terminal
nice -n 10 npm run test:browser # Chromium layout regressions, also included in npm test
nice -n 10 npm run test:captures # sequential inbox review captures (separate CI gate)
nice -n 10 npm run build      # emits skriftapp/buildapp/static/ (served by BuildController)
nice -n 10 npm run dev        # Vite dev server (proxy /api to a running skriftapp yourself)
```

Browser tests use the `playwright-core` dev dependency and require Chromium or
Chrome on `PATH`, or `CHROMIUM_PATH` set to its executable. The app image only
runs `npm run build`; it does not need a browser.

`test:captures` runs `captureInboxBadgeSum`, `captureTaskUnread` and
`captureWatching` against sample data in headless Chromium. It checks badge
values, fold behavior and watching controls, with no bridge or relay. The
runner removes inherited `BRIDGE_*` variables from each child. It prints and
retains a temporary output directory; pass `-- /path/to/captures` to choose one.
Direct script invocations also accept an output directory, otherwise they write
to their original `design/` directories.

CI runs the capture gate separately after Vitest. It stays out of `npm test`
because the scripts start multiple browser/server pairs and write review PNGs;
those serial captures should not compete with the parallel layout suite. A
failed assertion or capture fails the command by exit code. This is a DOM
assertion gate, not a pixel comparison against historical screenshots.

## Configuration

- `VITE_RELAY_URL` — the relay's WebSocket origin for `/ws/client`.
  Default `ws://localhost:18090` (local dev relay); production builds use
  `VITE_RELAY_URL=wss://relay.getbuild.ing npm run build`.
- `VITE_BUILD_CHANNEL` — the build-time channel. `dev` shows a persistent
  `Dev` badge and sets the title to `Build Dev — review is the product`.
  Unset or `prod` shows no badge and keeps the original title. The label comes
  from build configuration only, independent of connection or device state.

## Serving

`BuildController` serves `static/index.html` at `/app/` (substituting the
`{{USER0}}` avatar initial) and hashed assets at `/app/static/assets/*` with
immutable caching. The build output is gitignored — build it as part of the
image/deploy pipeline.

CSP needs: `script-src 'self'`, `style-src 'self' 'unsafe-inline'` (inline
style attributes), `connect-src 'self' data: <relay ws(s) origin>` (`data:`
because ghostty-web instantiates its wasm from a data: URL), `font-src 'self'`.

## Markdown rendering

All surfaces use `markdownHtml` in `src/core/markdown.js`. The renderer supports
aligned pipe tables, nested/ordered/task lists, ATX/setext headings, rules,
blockquotes, emphasis/strikethrough, literal code spans, backtick/tilde fences,
web links and Build references. GitHub-style additions include the five alerts,
footnotes, HTTP(S) images, angle autolinks, text-only `<kbd>` and native details.
`src/styles/markdown.css` shares typography and feature styling across chat,
tasks and document previews, using the light/dark theme tokens.

This is an intentionally safe subset, not a full CommonMark/HTML interpreter.
Footnote definitions are top-level, with four-space/tab continuation; identifiers
use letters, digits, underscores or hyphens. Details requires standalone exact
`<details>`/`</details>` lines and an immediate single-line `<summary>…</summary>`.
Keyboard notation accepts only exact attribute-free `<kbd>text</kbd>` with no
nested tags. Other HTML stays text. Relative/data URLs are refused; image URLs
also reject credentials. Existing link-title parsing refuses bracket characters.
Fences show their language label; registered Prism grammars highlight up to
4,096 characters, with longer/unknown code shown as escaped plain text to bound
rendering work. No additional dependencies are needed.

Every HTTP(S) image URL authored in markdown renders as a keyboard-accessible
“Load image from host” button with its alt text. No image element or request
exists until that button is activated; only that instance loads, and a cached
repaint resets consent. Same-origin URLs require the same activation because
their routes may redirect to external hosts. No HTTP attachment-route exception
exists. Build's attachment and cache loaders keep displaying their own
byte-backed images directly; authored `blob:` and `data:` sources stay refused.
All markdown images use `referrerpolicy="no-referrer"`. `markdownImages.js`
delegates activation and revalidates the URL before creating the image.
Images may be web-link labels: activating the load button does not follow the
enclosing link, and following that link does not load its image.
The CSP permits HTTPS images and same-origin HTTP images after activation; it
does not permit general external HTTP loading, and the renderer does not upgrade URLs.
The renderer's absence of an image element before activation is the consent
guard; CSP does not provide it.

Table prose wraps; numeric/measurement cells and right-aligned columns keep
their values together. `markdownTableFocus.js` measures rendered tables and
adds a keyboard stop only while a table actually overflows.
After the initial scan, only table additions and removals change observation;
streamed text and timer updates rely on ResizeObserver for actual size changes.

Pass a stable `taskLabelScope` for each document rendered alongside others;
this scopes checkbox labels and footnotes. Chat messages and task bodies/comments
do so. `markdownNavigation.js`, installed once by `main.js`, handles footnote
activation without replacing the SPA's hash route. It resolves within the
nearest document and moves keyboard focus as well as scroll position.

The fixture in `test/browser/markdownPolishFixture.mjs` covers every feature.
Capture it with headless Chromium (from `spa/`):

```bash
nice -n 10 node test/browser/captureMarkdownPolish.mjs /tmp/markdown-review after
# Optional fourth argument reads the original renderer/CSS from a Git commit:
nice -n 10 node test/browser/captureMarkdownPolish.mjs /tmp/markdown-review before 54e8e1d79
# Production other-agent chat, cached task comment, and Files plan document:
nice -n 10 node test/browser/captureMarkdownSurfaces.mjs /tmp/markdown-review
```

Both commands produce desktop, phone and 320px narrow captures in light/dark. The before mode
uses Vite's loader and `git show`; it does not change the checkout. Layout tests
add a 320px narrow-screen check, verify the timing table fits all three columns,
exercise local scrolling for genuinely wide tables/code, and keep loaded images
inside the message. See the Markdown Rendering
Security Checklist for the fixed output vocabulary and adversarial evidence.

## Mermaid diagrams

Complete `mermaid` fences in shared markdown render as local diagrams. The
library loads lazily, follows light/dark theme, and enhances cached documents
without a connection. Invalid or deliberately unsupported syntax retains its
escaped code and a small “Diagram unavailable” message.

Only text labels are supported: configuration/frontmatter, `%%` comments and
directives, HTML/entities/escapes, markdown labels, custom CSS, links and
image/icon or legacy sequence actor metadata are refused before rendering. No
diagram can load an authored image or stylesheet. Output SVG is rebuilt through
a narrow allowlist and uses application colors. See the
[dependency/security review](../planning/v2/Mermaid%20Dependency%20Review.md) for
the exact dependency, limits, audit findings and transient-DOM protections.
