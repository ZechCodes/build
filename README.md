# Build

**tmux for coding agents — in your browser, end-to-end encrypted, with git-native review.**

You set a goal from any device. An agent on *your* hardware writes a plan. You review the plan,
leave notes, approve. An agent builds. You review the diff, comment, approve. Build merges.
At any point you can drop into the agent's terminal — but you never have to.

Build does not run agents, host code, or see code. Agents run on the user's own machine via the
bridge; the relay moves ciphertext and nothing else. Build's job is **orchestration**: starting
work, watching it through git, and gating the transitions where human judgment matters.

The web app lets you choose **Claude Code** or **Codex CLI** per plan/run, including each
provider's model and reasoning-effort options. The selected CLI must already be installed and
authenticated on the machine running `build-bridge`.

Task intake is **goal-form + batched plan/diff comments** — there is deliberately no chat UI.

See [`planning/v2/`](planning/v2/) for the full scope, UI design brief, and roadmap, and
[`HANDOFF.md`](HANDOFF.md) for the current state and how to run everything.

## Architecture

```
                    getbuild.ing                relay.getbuild.ing
┌─────────┐  HTTPS ┌────────────┐  /internal/*  ┌────────────┐  wss ┌─────────┐
│ browser │◄──────►│ skriftapp  │◄──────────────│ Rust relay │◄────►│ bridge  │
│  (SPA)  │        │ api + SPA  │               │ ciphertext │      │ (user's │
└──┬─┬────┘        └─────┬──────┘               │    only    │      │  box)   │
   │ │                   ▼                      └────────────┘      └────┬────┘
   │ │             Postgres 16                        ▲                  │
   │ └────────── wss /ws/client ──────────────────────┘                  │
   └╌╌╌╌╌╌╌ WebRTC DataChannel (direct; Cloudflare TURN fallback) ╌╌╌╌╌╌╌┘
```

| Component | Where | What |
|---|---|---|
| `bridge/` | user machines | Rust device daemon: worktree-per-task, full-PTY harnesses, single `done` MCP tool, git-diff watcher, durable task store, E2EE transport, device pairing |
| `bridge/src/bin/relay.rs` | relay.getbuild.ing | Rust ciphertext-only broker: `/ws/device` (Ed25519 auth) + `/ws/client` (gateway-token auth); once a session upgrades to its DataChannel the relay carries signaling, presence and fallback only |
| `skriftapp/` | getbuild.ing | Python app server (Skrift): passkey auth, device registry/approval, gateway tokens, web push, the admin transport page (how sessions reach bridges: direct / TURN / relay), serves the SPA |
| `spa/` | built into skriftapp | Vite vanilla-ES-module web client — task board, plan/diff review, terminal drawer; all deps self-hosted, zero CDN |
| `desktop/` | user desktops | Sandboxed Electron client for the hosted SPA; connects to a separately installed bridge through the E2EE relay |
| `web/` | dev only | Node E2EE test/QA harnesses |
| `deploy/` | — | podman compose stack + k8s manifests and the cutover runbook |

The E2EE crypto layer lives in the separate
[`build-secure-transport`](https://github.com/ZechCodes/build-secure-transport) repo
(Python + JS bindings; the bridge carries an interop-verified Rust port).

**A second infrastructure party.** Browser and bridge negotiate a direct WebRTC DataChannel and
use Cloudflare TURN only when neither peer can hole-punch, which makes Cloudflare a second
infrastructure party beside the relay. Cloudflare sees TURN allocation source IPs and DTLS
ciphertext; under that DTLS is the same secretbox envelope the relay carries, so even a broken
DTLS session exposes no more than the relay already sees — session ids, sizes, timing — and
never plaintext or session keys. The peer's DTLS fingerprint travels inside the sealed session,
so neither Cloudflare nor anyone else on the path can substitute a peer. The direct path adds
the one exposure the relay path hid: each peer learns the other's IP. TURN credentials are
short-lived — their lifetime is `TTL_SECONDS` in `skriftapp/buildapp/ice_servers.py` — minted
per authenticated user by the api, and reach the bridge inside the sealed session; the TURN key
itself never leaves the api Secret.

## Devices

Every device you have paired shows up at once. The client opens a session to
each one, so the inbox and the projects rail list all of your machines' work
together and each row says which machine it is on. The device dropdown at the
top of the rail **filters** that list — **All devices**, or one machine — and
nothing else: it does not move where anything runs, and a branch or issue you
have open stays open whichever way the filter is set.

A machine that goes offline keeps its place: its rows stay in the rail, greyed
and marked offline, and its verbs come back the moment it reconnects. When no
machine can answer at all, Build waits for one rather than showing an empty
app.

Each device has its own settings page — the settings cog beside a device in the
dropdown opens it. That page is everything that belongs to that machine: its
projects folder, its projects list, **Add project**, agent modes, the default
harness and its isolation and triage settings. The device must be online to
open its page.

**Settings** (the account page) holds what is not any one machine's: the
**Creation device** — "New projects and captures go to" — plus agent defaults,
appearance, notifications, downloads, and your devices and keys.

## Device project folders

On a device's settings page, use **Choose folder…** to browse that device's
filesystem and select the folder where its new projects should be kept.

The choice is saved on that device and survives bridge restarts. It applies to
new repositories and clones; existing repositories and task worktrees stay
where they are. `BRIDGE_PROJECTS_DIR` supplies the initial default when no
projects folder has been saved in the bridge configuration.

## Adding projects

**Add project** on a device's settings page offers two choices, and adds the
project to that device. **Use existing folder** starts the directory browser in
that device's configured projects folder and opens the folder you choose. A
folder without Git opens in **Files**; it stays an ordinary folder until you
select **Initialize Git** on **Changes**. Initializing Git leaves your existing
files untracked so you can review them before committing.

**Create new project** asks for a name and an optional Git remote. Build creates
the repository inside that device's configured projects folder using `main` as
its initial branch. The optional remote is configured as `origin`; creation does
not clone or push it.

**New project** in the projects rail, and anything you capture with the compose
box, goes to the **Creation device** on Settings instead — those two are the
only places that pick a machine for you.

## Develop

```bash
cd bridge && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
cd skriftapp && uv run --frozen ruff check buildapp && uv run --frozen pytest buildapp
cd spa && npm run lint && npm test && npm run build
cd desktop && npm test && npm run pack
```

Full local stack (app + relay + bridge + scripted QA) via podman compose:
see [`deploy/README.md`](deploy/README.md). Production deploy:
[`deploy/k8s/CUTOVER.md`](deploy/k8s/CUTOVER.md).

## License

MIT (bridge / web client). The E2EE transport is published separately for auditability.
