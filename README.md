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
│  (SPA)  │        │ api + SPA  │               │ rendezvous │      │ (user's │
└──┬─┬────┘        └─────┬──────┘               │    only    │      │  box)   │
   │ │                   ▼                      └────────────┘      └────┬────┘
   │ │             Postgres 16                        ▲                  │
   │ └────── wss /ws/client (while negotiating) ──────┘                  │
   └╌╌╌╌╌╌ WebRTC DataChannels (direct; Cloudflare TURN fallback) ╌╌╌╌╌╌╌┘
```

Build hosts **authentication and rendezvous, and nothing else.** Conversations,
commits, diffs and files are large, and relaying any meaningful share of them is
the cost a hosted service must not carry. So the relay finds your machine and
carries the WebRTC negotiation; the browser closes that socket once the
DataChannels are open, and every application byte goes peer to peer from then
on. A machine that cannot be reached directly (or through TURN) is shown as
blocked, with the reason and a Retry — there is nothing underneath to fall back
to.

| Component | Where | What |
|---|---|---|
| `bridge/` | user machines | Rust device daemon: worktree-per-task, full-PTY harnesses, single `done` MCP tool, git-diff watcher, durable task store, E2EE transport, device pairing |
| `bridge/src/bin/relay.rs` | relay.getbuild.ing | Rust ciphertext-only broker: `/ws/device` (Ed25519 auth) + `/ws/client` (gateway-token auth). Carries session setup and `rtc.*` signaling and refuses everything else; presence and transport keys are the api's |
| `skriftapp/` | getbuild.ing | Python app server (Skrift): passkey auth, device registry/approval, gateway tokens, web push, the admin transport page (how sessions reach bridges: direct / TURN / relay), serves the SPA |
| `spa/` | built into skriftapp | Vite vanilla-ES-module web client — task board, plan/diff review, terminal drawer; all deps self-hosted, zero CDN |
| `desktop/` | user desktops | Sandboxed Electron client for the hosted SPA; connects to a separately installed bridge through the E2EE relay |
| `web/` | dev only | Node E2EE test/QA harnesses |
| `deploy/` | — | podman compose stack + k8s manifests and the cutover runbook |

The E2EE crypto layer lives in the separate
[`build-secure-transport`](https://github.com/ZechCodes/build-secure-transport) repo
(Python + JS bindings; the bridge carries an interop-verified Rust port).

**A second infrastructure party.** Browser and bridge negotiate direct WebRTC DataChannels and
use Cloudflare TURN only when neither peer can hole-punch, which makes Cloudflare a second
infrastructure party beside the relay. Cloudflare sees TURN allocation source IPs and DTLS
ciphertext; under that DTLS is the same secretbox envelope the relay carries during negotiation,
so even a broken DTLS session exposes no more than the relay already saw — session ids, sizes,
timing — and never plaintext or session keys. The peer's DTLS fingerprint travels inside the sealed session,
so neither Cloudflare nor anyone else on the path can substitute a peer. The direct path adds
the one exposure the relay path hid: each peer learns the other's IP. TURN credentials are
short-lived — their lifetime is `TTL_SECONDS` in `skriftapp/buildapp/ice_servers.py` — minted
per authenticated user by the api, and reach the bridge inside the sealed session; the TURN key
itself never leaves the api Secret.

## Devices

Every device you have paired shows up at once. The client opens a session to
each one, so the inbox and the projects rail list all of your machines' work
together. Where two machines hold projects with the same name, that name
carries the machine beside it — on the rows themselves, in the rail's project
headings and in the toolbar's project menu — so the two are never confused.
The device dropdown at the foot of the rail, beside **Account**, **filters**
that list — **All devices**, or one machine — and nothing else: it does not
move where anything runs, and a branch or issue you have open stays open
whichever way the filter is set.

A machine that goes offline keeps its place: its rows stay in the rail, greyed
and marked offline, and its verbs come back the moment it reconnects. When no
machine can answer at all, Build waits for one rather than showing an empty
app.

Each device has its own settings page — the settings cog beside a device in the
dropdown opens it. That page is everything that belongs to that machine: its
projects folder, its projects list, **Add project**, agent modes, the default
harness, its **Work isolation** choice — git worktrees or **Rift
(copy-on-write)**, as below — and its triage settings. The device must be online
to open its page.

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

## Projects and workspaces

A project brings together one or more source directories on a device. Add local
directories, Git repositories, or remote repositories that Build clones. Each
source keeps its own files and Git state; a project may also include ordinary
directories that have no Git repository.

When Build creates a workspace, it materializes each source separately. A
non-Git source is copied using the selected backend: a regular copy with the
worktree backend, or the installed Rift CLI with the Rift backend, falling
back to a regular copy when Rift is unavailable or cannot create the copy safely.
Git sources are checked out independently.
An older project whose root is itself a Git checkout remains a single-directory
project represented by `.` and is never moved.

Directory tabs follow the workspace menu and collapse into a directory menu
on phones. Selecting a directory controls the Files, Changes, and commit views.
All changes includes unpushed commits and uncommitted edits, using locally known
remote history as its baseline. A searchable selector above the commit list
separates branches and tags and identifies remote branches and available pulls.
Switching a ref performs a real checkout in that source. Compatible
uncommitted edits remain in place; if a checkout would overwrite or otherwise
lose local changes, Build leaves the source untouched and reports Git's reason.
Selecting a tag results in a detached HEAD and the interface says so.

The directory picker can create a new folder while adding project sources.
From a workspace directory, Initialize Git offers the workspace copy, the
original project source, or both. Initializing the source enables Git for future
workspaces; initializing only the copy leaves the original source unchanged.
When they are separate folders, initializing both creates independent
repositories and preserves the files in each. If the workspace and source are
the same folder, Git is initialized there once.

Terminals belong to the workspace, not to the selected source or ref. A new
terminal starts at the workspace root and remains open when the selected source
or its ref changes. A terminal that has changed directory stays where its shell
put it; selecting a source does not change its working directory.

Finishing a workspace verifies that every Git source is pushed to its remote,
then retains the workspace checkout and all of its files. Build never deletes a
workspace as part of Finish. It retains per-source recovery state so an
interrupted operation can resume safely. Later edits to a project's source list
affect new workspaces only: an existing workspace continues from its recorded
source configuration.

For the complete workspace behavior, see
[`planning/v2/workspaces.md`](planning/v2/workspaces.md).

**New project** in the projects rail, and anything you capture with the compose
box, goes to the **Creation device** on Settings instead — those two are the
only places that pick a machine for you.

## Work isolation

Build uses Git worktrees by default. For copy-on-write checkouts, install the
[Rift CLI](https://github.com/anomalyco/rift#install) on the machine running the
bridge and make `rift` available on the bridge's `PATH`. Select **Rift
(copy-on-write)** in Work isolation settings, either as that device's default
or as a project override. The choice applies to new checkouts.

Rift owns filesystem cloning and snapshot creation. Build requests full copies
to retain ignored build caches, skips Rift hooks, and checks out the task's
branch for Git sources. Workspace completion verifies pushes and retains files
with either backend. Build does not install or update Rift for you.

The first Rift task initializes its source project. On Btrfs, Rift may convert
the source directory into a subvolume; on other supported filesystems it
registers the directory in place. Git sources must have their own `.git`
directory; ordinary directories need no Git metadata. The checkouts folder
must be outside the source. Filesystem support and
initialization errors are reported by Rift when creating the checkout.

Build keeps a private Rift registry under each project's checkouts folder at
`.rift/registry.sqlite`, so its garbage collection does not touch workspaces
registered by other Rift users. A project already registered in a different
Rift registry cannot be initialized in Build's registry; Build leaves its
marker untouched and reports the conflict. Keep the Rift CLI installed while
Build has Rift checkouts to manage.

## Develop

```bash
cd bridge && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
cd skriftapp && uv run --frozen ruff check buildapp && uv run --frozen pytest buildapp
cd spa && npm run lint && npm test && npm run build
cd desktop && npm test && npm run pack
```

The bridge's ICE agent is tuned by three variables (strict P2P transport spec,
rule 8): `BRIDGE_ICE_POLICY` (`all`, the default, or `direct-only` — strip the
browser's TURN servers and refuse relay candidates), `BRIDGE_ICE_RELAY_MIN_WAIT_MS`
(how long a TURN pair waits before it may be accepted so a slower direct pair can
win; default `1500`, `0` for no wait) and `BRIDGE_ICE_INTERFACES` (a comma list of
interfaces to gather host candidates on, e.g. `tailscale0,eth0`; unset means every
non-loopback interface, IPv4 and — where the machine has an address of its own —
IPv6). Every `BRIDGE_*` variable is listed in `bridge/src/main.rs`.

Full local stack (app + relay + bridge + scripted QA) via podman compose:
see [`deploy/README.md`](deploy/README.md). Production deploy:
[`deploy/k8s/CUTOVER.md`](deploy/k8s/CUTOVER.md).

## License

MIT (bridge / web client). The E2EE transport is published separately for auditability.
