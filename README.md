# Build

**tmux for coding agents — in your browser, end-to-end encrypted, with git-native review.**

You set a goal from any device. An agent on *your* hardware writes a plan. You review the plan,
leave notes, approve. An agent builds. You review the diff, comment, approve. Build merges.
At any point you can drop into the agent's terminal — but you never have to.

Build does not run agents, host code, or see code. Agents run on the user's own machine via the
bridge; the relay moves ciphertext and nothing else. Build's job is **orchestration**: starting
work, watching it through git, and gating the transitions where human judgment matters.

See [`planning/v2/`](planning/v2/) for the full scope and UI design brief.

## Architecture

```
┌─────────────┐   E2EE relay    ┌──────────────┐   spawns    ┌──────────────┐
│  Web client │◄───ciphertext──►│    bridge    │────PTY─────►│ agent harness │
│  (browser)  │                 │ (user's box) │◄────MCP─────│  (worktree)   │
└─────────────┘                 └──────┬───────┘             └──────────────┘
                                       │ watches
                                       ▼
                                  git worktree
```

The system spans four repos:

| Component | Repo | Status |
|---|---|---|
| E2EE crypto (Python + JS) | `build-secure-transport` | built |
| Ciphertext-only relay | `build-relay` | built |
| Device daemon (v2, Rust) | **this repo → `bridge/`** | in progress |
| Web client (React) | **this repo → `frontend/`** | v2 rebuild pending |

## bridge/ (Rust device daemon)

Owns worktrees, spawns harnesses in full PTYs, serves the single-tool (`done`) MCP server,
watches git, runs git operations, and talks to the relay. Built TDD-first.

```bash
cd bridge
cargo test
```

## License

MIT (bridge / web client). The E2EE transport is published separately for auditability.
