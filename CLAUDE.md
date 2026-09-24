- Always use TDD
- Always commit your changes as you make them
- Always ensure security checklists are 100/100
- Run semgrep and gitleaks before every commit
- Keep the full project in mind by reading planning/v2/ — `E2EE Platform Scope.md` (architecture, task lifecycle, MCP `done` tool) and `UI Design Brief for E2EE Platform.md` (web client UX)

## What this repo is (v2)

A full break from v1. Build is "tmux for coding agents" — E2E encrypted, git-native review, task-oriented (not channels), full PTY (no harness SDKs).

The system spans four repos:

- **build-secure-transport** (separate repo) — the E2EE crypto layer. Python + JS bindings. Already built.
- **build-relay** (separate repo) — the ciphertext-only WebSocket relay. Already built.
- **build-bridge** (separate repo) — the v1-era Python device daemon (channel-oriented). Being superseded by `bridge/` here.
- **this repo** — hosts the v2 Rust `bridge/` (device daemon: worktrees, full PTY harnesses, single `done` MCP tool, git-diff watcher, task lifecycle) and `frontend/` (the React/Vite/xterm/Monaco web-client base, to be rebuilt for the v2 task board / plan / diff / terminal UX).

## Design rules (from the scope doc)

1. Everything is files + git + PTY. Every phase is an agent in a PTY operating on files.
2. Enforcement by observation, not permission. Make what the agent did legible (git diff); the human decides.
3. The terminal is the basement: always accessible, never the default view.
4. Review (plan + diff) is the product.

## Complexity gates

Every tier caps how complex one function may be, and CI runs the cap:

| tier | rule | gate |
| --- | --- | --- |
| `skriftapp/` | ruff `C901`, max-complexity 10 | `uv run --frozen ruff check buildapp` |
| `bridge/` | clippy `cognitive_complexity`, threshold 15 (`bridge/clippy.toml`) | `nice -n 10 cargo clippy --all-targets -- -D warnings` |
| `spa/` | eslint `complexity`, max 10 (`spa/eslint.config.js`) | `nice -n 10 npm run lint` |
| `*.sh` | shellcheck | `git ls-files '*.sh' \| xargs shellcheck` |

Gates run under `nice -n 10` because they run on the same machine as the
user's bridge and the user's apps, and under `nice -n 10` (or in the agents'
slice) they lose to both.

The functions that were already over the cap when the gates landed carry a
one-line ratchet annotation — `# noqa: C901`, `#[allow(clippy::cognitive_complexity)]`,
`// eslint-disable-next-line complexity` — each naming its score and what would
retire it. They are debt, listed so main is green, not permission.

**The rule: no PR adds to a ratchet list.** Removing one is welcome in its own
commit. A new function over the cap is split, not annotated.

## bridge/ (Rust)

```
nice -n 10 cargo test        # TDD: write the test first, watch it fail, make it pass
nice -n 10 cargo clippy --all-targets -- -D warnings
nice -n 10 cargo fmt
```
