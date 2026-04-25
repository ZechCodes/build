# 08 — Conventions

These are the rules v2 code follows. Breaking them is a review comment.

## File layout

See [01-architecture.md](01-architecture.md) for the canonical tree.
Summary:

- `core/` — bus, store base, logger. No domain knowledge.
- `transport/` — wire protocol handlers. No DOM, no store imports (only
  bus).
- `domain/` — stores. Own slices. Subscribe to bus; expose mutations.
- `channel/` — `Channel`, `ChannelRegistry`, per-channel views.
- `shell/` — app chrome. Routing, layout, tabs, rail, dropdowns.
- `util/` — pure helpers. No side effects on import.
- `styles/` — one stylesheet per view family plus tokens/base/layout.

## Naming

- **Files**: kebab-case. `messages-store.js`, `chat-view.js`,
  `e2ee-dispatcher.js`.
- **Exported names**: camelCase for values, PascalCase for classes.
  `messagesStore`, `ChatView`, `bus`, `channelRegistry`.
- **Bus event types**: dot-separated lowercase, domain first.
  `message.received`, `agent.tool_use`, `channel.upserted`,
  `intent.send_message`.
- **Store methods**: `get`, `list`, `upsert`, `remove`, `subscribe`,
  plus domain verbs (`append`, `markRead`, `setActive`). Avoid `update` —
  prefer specific verbs.
- **DOM selectors**: prefer `[data-view="..."]` and `[data-slot="..."]`
  attributes for view-owned regions. IDs only for singletons
  (`#app`, `#chat-overlay`).

## Imports

- **No `window.*` bridges.** Ever.
- **No re-export hubs.** (v1's `legacy.js` is the anti-pattern. Don't
  replace it with a `v2-index.js`.)
- **Cross-layer imports follow the arrow in
  [01-architecture.md](01-architecture.md):**
  - Transport may import `core/bus.js`.
  - Stores may import `core/bus.js`, `core/store.js`.
  - Channel may import stores, bus, transport (for `forChannel`).
  - Views may import stores, bus.
  - Shell may import `uiStore`, `core/bus.js`, `channelRegistry`, router.
- **Never**: store importing store, view importing another view (except
  parent→child within a view family), transport importing store, store
  importing transport.

## State discipline

- **Per-channel view state** (resets on close): `Channel.viewState`.
- **Domain data indexed by channelId**: domain store.
- **Session-global UI state**: `uiStore`.
- **Never stash state on `window`, on DOM elements as properties, or in
  module-level `let` except inside a store file.**

## Error handling

- **Transport errors**: logged, surfaced on `bus` as `transport.error`
  (not yet in vocabulary; add when needed).
- **Store mutations that would violate invariants**: throw. Calling code
  is wrong.
- **View `render()` errors**: caught at the view boundary, logged, show a
  fallback "failed to render" placeholder in the view's root. One broken
  view does not take down the app.
- **Bus subscriber errors**: swallowed + logged by the bus itself (see
  [04-transport.md](04-transport.md)).

## Logging

Use `core/log.js` scoped loggers:

```js
import { log } from '../core/log.js';
const plog = log('e2ee');
plog.info('connected', deviceId);
plog.warn('unknown wire event', type);
```

Scopes: `transport`, `e2ee`, `sse`, `store:<name>`, `view:<name>`,
`channel`, `shell`, `router`.

Log levels: `debug` (off in prod), `info`, `warn`, `error`. No emoji. No
per-line formatting — the logger handles that.

## Comments

- **Why, not what.** The code tells the reader what happens; comments
  explain why it's that way when the why is non-obvious.
- **No TODO drift.** A TODO older than a wave gets a linked issue or gets
  deleted.
- **No "used by X" or "removed Y" comments.** That's what git history is
  for.

## Testing

- **Unit tests**: stores, bus, transport dispatcher, router.
- **View tests**: jsdom or Playwright, exercising `activate`/`deactivate`,
  `render()`, and intent emissions.
- **Smoke test**: Playwright script that logs in, activates the channel
  panel, selects a channel, sends a message via a synthetic E2EE
  instance, asserts the message renders.
- **No test fixtures in source tree**: `frontend/tests/` only.

## CSS

- **One stylesheet per view family.** Chat, files, terminal, console,
  rail, channel panel, tokens/base/layout.
- **CSS custom properties in `tokens.css`** — colors, spacing, type
  scale. No hex values inline in other stylesheets.
- **Dark mode** via `@media (prefers-color-scheme: dark)` *or* a
  `data-theme="dark"` attribute on `<html>`. Pick one (TBD in Wave 3).
- **No `!important`** except for utility classes explicitly marked as
  overrides.
- **No CSS-in-JS.** Vanilla stylesheets, bundled by esbuild.

## HTML templates

`dashboard.html` is the only HTML template. Everything else is built
in JS. If you catch yourself adding another HTML template, ask whether
it's a view's `render()` output instead.

## Security

- **Escape user content.** `escapeHtml()` for any user string going into
  `innerHTML`. Prefer `textContent` when no markup is needed.
- **Markdown rendering** is a trusted source (agent-authored), but
  still pass through the renderer (never `innerHTML` raw markdown).
- **File preview** stays in an iframe with the existing `/preview-frame`
  endpoint's CSP.
- **No `eval`, no `new Function`, no inline event handlers in `innerHTML`.**
  Attach listeners post-render.

## Performance budgets

Soft targets; revisit after Wave 4.

- Initial JS bundle: < 300 KB min+gzip.
- Time to first channel panel render: < 200 ms after bundle load.
- Channel switch (ChatView mount): < 50 ms p95.
- Sustained scroll at 1000 messages: 60 fps on a 2019 laptop.

If a view becomes hot, measure before optimizing. Don't prematurely
virtualize lists that are small in practice.
