# Dashboard v2 — Planning

This directory is the guiding light for the Dashboard v2 rewrite. v2 is being
built in parallel with v1. v1 keeps serving users while v2 is developed at
`/dashboard-v2/`; when v2 reaches parity, the route is promoted to `/dashboard/`
and v1 is deleted.

## Why a rewrite

v1 is functional but not fully operational after an aggressive refactor from a
single 8000-line template + monolithic JS. The rewrite broke the monolith into
folders but kept the monolith's **control flow**: one flat mutable state
object, a re-export hub (`legacy.js`), window bridges, and E2EE handlers that
mix transport, domain, and view concerns. The result is:

- No channel lifecycle → per-channel view state leaks between channels.
- No subsystem isolation → `selectChannel()` is 187 lines of cross-cutting wiring.
- No message bus → adding a subscriber means editing every handler.

See [00-diagnosis.md](00-diagnosis.md) for the full teardown.

## What v2 is

Four layers, strictly separated:

```
Shell         — routing, layout, tabs, rail                (no domain)
Channel       — one instance per open channel; owns view state, lifecycle
Domain stores — devices, channels, messages, files, terminal, tasks, ...
Transport     — E2EE, SSE, REST; publishes typed events onto a bus
```

See [01-architecture.md](01-architecture.md) for the contract between layers.

## How to use this directory

1. Read in order: `00` → `08`. Each doc is short.
2. Before coding any new v2 file, check the doc that owns its layer. If the
   rule isn't there, add it — these docs are the source of truth.
3. When a rule conflicts with reality, update the doc first, then the code.

## Docs

- [00-diagnosis.md](00-diagnosis.md) — symptoms and root causes in v1.
- [01-architecture.md](01-architecture.md) — 4-layer model, rules, key abstractions.
- [02-stores.md](02-stores.md) — domain stores, APIs, ownership.
- [03-channel-lifecycle.md](03-channel-lifecycle.md) — Channel object, registry, activation.
- [04-transport.md](04-transport.md) — event bus, E2EE/SSE dispatcher, vocabulary.
- [05-views.md](05-views.md) — view contracts, mount/unmount discipline.
- [06-shell.md](06-shell.md) — routing, layout, tabs, rail.
- [07-migration.md](07-migration.md) — parallel build, phased delivery, cutover.
- [08-conventions.md](08-conventions.md) — file layout, naming, imports, testing.

## Status

Wave 0 (scaffold): v2 route, template, esbuild entry in place. Bootstrap renders
"Dashboard v2" placeholder. No domain code yet.

Update this section when waves complete. See [07-migration.md](07-migration.md)
for the wave plan.
