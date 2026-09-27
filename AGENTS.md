# Agent instructions

Everything an agent working in this repository needs to follow. `CLAUDE.md`
imports this file and holds nothing else; edit this one.

## Read first

**Read [`ARCHITECTURE.md`](ARCHITECTURE.md) before changing `spa/` or `bridge/`.**
It describes the code as it is on main. Per area:

| You are changing | Read |
| --- | --- |
| anything in `bridge/` | [The bridge](ARCHITECTURE.md#the-bridge), then the subsection for the area |
| an RPC verb or push event | [RPC and push events](ARCHITECTURE.md#rpc-and-push-events), [Wire versioning and capabilities](ARCHITECTURE.md#wire-versioning-and-capabilities) |
| harnesses, agents, MCP tools | [Harnesses and the agents' slice](ARCHITECTURE.md#harnesses-and-the-agents-slice), [MCP tools](ARCHITECTURE.md#mcp-tools) |
| transport, relay, WebRTC | [System topology](ARCHITECTURE.md#system-topology), [Relay and direct connection](ARCHITECTURE.md#relay-and-direct-connection) |
| anything in `spa/` | [The SPA](ARCHITECTURE.md#the-spa), and always [Render from cache](ARCHITECTURE.md#render-from-cache) |
| SPA connection or version handling | [Connection state machine](ARCHITECTURE.md#connection-state-machine), [Capability gating](ARCHITECTURE.md#capability-gating) |
| colours or theme | [Theming](ARCHITECTURE.md#theming) |
| "where does X go?" | [Where to look](ARCHITECTURE.md#where-to-look) |

Keep the full project in mind by reading, for intent,
[`planning/v2/E2EE Platform Scope.md`](<planning/v2/E2EE Platform Scope.md>)
(architecture, task lifecycle) and
[`planning/v2/UI Design Brief for E2EE Platform.md`](<planning/v2/UI Design Brief for E2EE Platform.md>)
(web client UX). The other specs and plans live beside them in
[`planning/v2/`](planning/v2/). They record intent and can lag behind: **where a
spec disagrees with the code or `ARCHITECTURE.md`, the code and
`ARCHITECTURE.md` win.** Build what was asked, then amend the spec to match.

## About this project

Build is an agentic IDE for developers, meant to be easy to contribute to: it
lets a developer monitor agents running on their own machine, track tasks,
create plans, review changes and make their own changes, from a browser or the
desktop app, end-to-end encrypted.

Design rules:

1. Everything is files + git + agent processes on the user's machine.
2. Enforcement by observation, not permission. Make what the agent did legible
   (the git diff); the human decides.
3. The terminal is the basement: always accessible, never the default view.
4. Review (plan + diff) is the product.
5. The bridge is RPC + push events. Product logic lives in the SPA; logic that
   must run while no client is connected stays in the bridge as a service,
   isolated from the RPC and event layer.
6. Every SPA view paints from the cache and never from connection state. Pulls
   and pushes write to the cache, then the view redraws (see Working rules).

## Working rules

- Always use TDD: write the test first, watch it fail, make it pass.
- Always commit your changes as you make them.
- Run gitleaks before every push and semgrep before you post ready (commands below). The roll gate runs both again on every merge.
- Security checklists must be 100/100. The feature checklists live in
  `planning/v2/*Security Checklist.md`.
- One task per branch, branched from `main`. Nothing merges without a review.
- **SPA rendering draws from the cache, never from connection state.** Every
  view renders from the local cache and assumes it is up to date. Nothing
  draws based on the status of a connection or is aware of the connection
  state machine's status, unless a render state showing that status is truly
  needed (generally never). While a device is connecting, everything renders
  from the local cache as if it were connected. New code follows the rule. The
  places today's code departs from it are listed under "Current exceptions" in
  [Render from cache](ARCHITECTURE.md#render-from-cache). They are not
  precedent.
- Never run `npm ci` in `spa/`: `spa/package-lock.json` is gitignored because
  `@build/secure-transport` is a `file:` dependency with a machine-specific
  path. Use `npm install --legacy-peer-deps`, and never commit that lockfile.
  (`landing/` and `desktop/` do commit their lockfiles and use `npm ci`.)

## Code rules

Code must be clean and readable. Names should be descriptive and self
documenting. Everything should be DRY when it makes sense. Polymorphism should
be the preferred approach over conditionals; cyclomatic complexity must be kept
low. Favor deep modules with narrow interfaces for isolation and
understandability, but never let functions, classes, files, etc. become
excessively long.

## Complexity gates

Every tier caps how complex one function may be, and CI runs the cap:

| tier | rule | gate |
| --- | --- | --- |
| `skriftapp/` | ruff `C901`, max-complexity 10 | `uv run --frozen ruff check buildapp` |
| `bridge/` | clippy `cognitive_complexity`, threshold 15 (`bridge/clippy.toml`) | `nice -n 10 cargo clippy --all-targets -- -D warnings` |
| `spa/` | eslint `complexity`, max 10 (`spa/eslint.config.js`) | `nice -n 10 npm run lint` |
| `*.sh` | shellcheck | `git ls-files '*.sh' \| xargs shellcheck` |

The functions that were already over the cap when the gates landed carry a
one-line ratchet annotation — `# noqa: C901`,
`#[allow(clippy::cognitive_complexity)]`,
`// eslint-disable-next-line complexity` — each naming its score and what would
retire it. They are debt, listed so main is green, not permission. The counts
are pinned by `bridge/tests/complexity_ratchet.rs` and
`spa/test/complexityRatchet.test.js`.

**The rule: no PR adds to a ratchet list.** Removing one is welcome in its own
commit. A new function over the cap is split, not annotated.

## Gates

Run every gate under `nice -n 10`: gates run on the same machine as the bridge
and apps they test, and niced they lose to both. **Judge every gate by its exit
code**, never by grepping its summary: a suite can print "passed" and still
exit non-zero.

```bash
cmd > /tmp/gate.log 2>&1; echo "exit=$?"
```

| tier | from | commands |
| --- | --- | --- |
| bridge | `bridge/` | `nice -n 10 cargo test` · `nice -n 10 cargo clippy --all-targets -- -D warnings` · `nice -n 10 cargo fmt --check` |
| bridge vendored webrtc | `bridge/` | `nice -n 10 cargo test --locked --manifest-path vendor/Cargo.toml --lib` (the patched webrtc on the patched rtc, `bridge/vendor/README.md`) |
| SPA | `spa/` | `nice -n 10 npm install --legacy-peer-deps` (when `package.json` changed) · `nice -n 10 npm run lint` · `nice -n 10 npm test` · `nice -n 10 npm run build` |
| skriftapp | `skriftapp/` | `nice -n 10 uv run --frozen ruff check buildapp` · `nice -n 10 uv run --frozen pytest buildapp -q` |
| landing | `landing/` | `nice -n 10 npm ci` · `nice -n 10 npm test` (builds the Astro page, then reads it back) |
| landing in a browser | repo root | `skriftapp/.venv/bin/python scripts/preview-landing.py`, then `CHROMIUM_PATH=/usr/bin/chromium nice -n 10 node web/landing-check.mjs` |
| desktop | `desktop/` | `nice -n 10 npm test` |
| shell | repo root | `git ls-files '*.sh' \| nice -n 10 xargs shellcheck` |

The SPA needs a checkout of
[`build-secure-transport`](https://github.com/ZechCodes/build-secure-transport)
beside the repo root (`spa/package.json` resolves it as
`file:../../build-secure-transport/js`); without it `npm run build` cannot
resolve `@build/secure-transport` and vitest fails wholesale. `npm test`
includes the Chromium browser tests (`spa/test/browser/`), which need Chromium
on `PATH` or `CHROMIUM_PATH`. Run `npx vitest` only from inside `spa/`, so the
repo's own vitest runs.

The scans (gitleaks before each push, semgrep before you post ready):

```bash
nice -n 10 semgrep --config auto --error <changed js/mjs/css files>
nice -n 10 gitleaks git --no-banner --redact --log-opts='main..HEAD' .
git diff --check main..HEAD
```

semgrep only scans files git tracks; `git add` a new file before scanning it.

## Orchestration

The primary/top level agent should act as the orchestrator and rely on
sub-agents for planning, implementation, periodic validation, and simple
choreographed tasks.

Astra is the big brains and should only be used for planning and review at
critical junctures.

Sol is the workhorse that does implementation and most review/validation work.
It can even do scoped planning tasks.

Terra is good for quick tasks, lookups, etc. It shouldn't be used for most
implementation tasks. Changing values, scoped refactors, etc. are ok.

Luna is a fast worker that can take a well choreographed job. Use it for
deploying changes, monitoring systems, etc. reporting back with pertinent
information.
