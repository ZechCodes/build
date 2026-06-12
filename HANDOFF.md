# Build v2 — Local Platform Handoff

Status: **running locally via podman compose and verified end-to-end.** This doc
is the hand-off for your final pass.

## Run it

```bash
podman compose up -d --build relay bridge web   # bring the stack up
podman compose --profile qa run --rm qa         # 15-check end-to-end verification
open http://localhost:8080                       # the browser client UI
podman compose down                              # tear down
```

Three services on one network, plus a one-shot `qa` verifier:

| Service | What it is | Port |
|---|---|---|
| `relay` | Rust opaque-envelope forwarder (dev stand-in for `build-relay`) | 8799 |
| `bridge`| Rust device daemon: orchestrator + worktrees + diff + E2EE, on a sample `/repo` | — |
| `web`   | Static server for the browser client (`index.html` + `client.mjs`) | 8080 |
| `qa`    | Runs `qa.mjs` (real browser-client logic) against the stack | — |

The data path is **browser → relay → bridge → relay → browser, fully E2E
encrypted.** The relay routes by `session_id` and forwards opaque envelopes
(`{version, session_id, route_to, nonce, ciphertext}`); it holds no session key
and never decrypts.

## What was verified (QA results)

`podman compose --profile qa run --rm qa` → **15/15 checks pass**, in-network:

- `ping` round-trips over the encrypted channel
- **Standard lifecycle**: dispatch → `plan_review` (plan readable, mentions the goal)
  → `approve_plan` → `review` (diff shows the produced file, non-empty patch)
  → `approve_merge` → `merged`
- **Quick task**: dispatch → `review` (planning skipped) → `merged`
- **Parallel tasks**: distinct `build/<slug>` branches, both on the board
- **Abandon**: reaches `abandoned` (worktree removed, branch kept)
- **Errors**: unknown method and missing params return clean errors, no crash

Verified directly in the running containers:

- Merges land for real on `main`: `git log` shows `Build: Add a greeting banner`
  and `Build: Quick fix typo` merge commits; `result.txt` present on `main`.
- Branches kept after merge/abandon (`build/parallel-task-a`, `build/parallel-task-b`).
- Web UI serves: `index.html` → HTTP 200 `text/html`, `client.mjs` → HTTP 200
  `text/javascript`.

The Rust suite is green too: **59 unit/integration tests**, a live Rust↔Python
crypto interop test, and a browser↔relay↔bridge end-to-end test. `cargo clippy
-D warnings` and `cargo fmt --check` clean; every commit gitleaks-scanned. CI runs
all of it plus the browser round-trip.

## Architecture (4 repos)

```
┌─────────────┐   E2EE relay    ┌──────────────┐   spawns    ┌──────────────┐
│  web client │◄───ciphertext──►│    bridge    │────PTY─────►│ agent harness │
│  (browser)  │                 │ (this repo)  │◄────MCP─────│  (worktree)   │
└─────────────┘                 └──────┬───────┘             └──────────────┘
                                       ▼ watches git worktree
```

- `bridge/` — Rust device daemon (this repo). Modules: `task` (lifecycle),
  `worktree`, `pty` (full-PTY harness, no SDK), `mcp` (single `done` tool),
  `diff`, `templates`, `orchestrator` (the spine), `transport` (E2EE binding via
  dryoc), `relay` (E2EE WS client), `app` (the orchestrator-backed RPC).
- `web/` — browser client (`client.mjs` runs in-browser and in the Node harness).
- `build-secure-transport` (separate repo) — audited E2EE crypto (Python + JS);
  the bridge speaks the same protocol via its Rust port, verified by live interop.
- `build-relay` (separate repo) — the production relay. The dev `relay` here is a
  minimal stand-in; the **device-side frame contract is identical**, so the bridge
  connects to either unchanged.

## The QA scripted agent (important context)

So the lifecycle runs locally without an LLM, the bridge runs with
`BRIDGE_QA_AGENT=1`: a deterministic *scripted agent* writes the plan/code files a
real agent would and reports `done` — exercising the **real** orchestrator path
(git worktrees, diff, merge, lifecycle). The only thing simulated is the agent's
authorship. Drop the flag and point `BRIDGE_REPO` at a real repo for real CLI
agents in PTYs.

## Known gaps — for your final pass

These are deliberately out of the local-demo scope; none require rearchitecting:

1. **Real `build-relay`** — swap the dev `relay` for the production relay
   (Postgres + Redis). The bridge's `/ws/device` side already matches it; the
   open item is the relay's **client/browser endpoint** + the web client wiring to
   it, plus device **registration/pairing** (`POST /api/devices/register` + SSE
   approval) and seeding an approved device.
2. **TLS / `wss`** — the relay client connects over `ws://`. Add a rustls feature
   to `tokio-tungstenite` for the deployed `wss://` relay.
3. **Real LLM harness + MCP `done` forward** — the daemon needs a per-task control
   socket so the `build-bridge mcp` shim can forward an agent's real `done` call to
   the orchestrator. The `mcp` module (server) and `pty` (spawn) are built; the
   socket/forward is the missing seam.
4. **Persistence** — task state is in-memory (resets on bridge restart). Persist
   the task store for durability across restarts.
5. **The production React UI** — `frontend/` (kept) is the base; `web/client.mjs`
   is the reusable E2EE session core to build it on.
6. **Security review** — the device auth (Ed25519-signed challenge) and the relay
   client deserve a focused review before production; the crypto binding is a port
   of the audited protocol and is interop-verified, but is not itself audited.

## File map (added this session)

```
bridge/Containerfile, bridge/bridge-entrypoint.sh, bridge/.dockerignore
bridge/src/app.rs            orchestrator-backed E2EE app RPC + QA agent
bridge/src/main.rs           `build-bridge serve`
bridge/src/bin/relay.rs      standalone dev relay
web/client.mjs               browser client (openSession + RPC)
web/qa.mjs                   end-to-end QA harness
web/serve.mjs                static server
web/Containerfile, web/.dockerignore
podman-compose.yml           the stack
```
