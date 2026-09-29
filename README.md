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
[`deploy/README.md`](deploy/README.md) for how to run the whole stack.

Contributing? Start at [Getting started](#getting-started), then
[Testing](#testing) and [Contributing](#contributing).
[`ARCHITECTURE.md`](ARCHITECTURE.md) explains how the bridge and the web client
are built.

## Brand assets

Official SVG and PNG artwork lives in [`assets/brand/`](assets/brand/README.md),
including the transparent mark and black-on-mint, mint-on-black, and black-on-white
variants. That directory also documents how to regenerate the website, SPA, and
desktop icons from the shared artwork.

## Architecture

How each piece is built inside — the bridge's RPC and push model, its state,
stores, harnesses and MCP tools; the web client's cache, connection state
machine and surfaces — is in [`ARCHITECTURE.md`](ARCHITECTURE.md). The overview:

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
move where anything runs, and a branch or task you have open stays open
whichever way the filter is set.

A machine that goes offline keeps its place: its rows stay in the rail, greyed
and marked offline, and its verbs come back the moment it reconnects. When no
machine can answer at all, Build waits for one rather than showing an empty
app.

Each device has its own settings page — the settings cog beside a device in the
dropdown opens it. That page is everything that belongs to that machine: its
projects folder, its projects list, **Add project**, agent modes, the default
harness, its **Work isolation** choice — git worktrees or **Rift
(copy-on-write)**, as below. The device must be online
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

## Install

Public installers detect macOS or Linux and select the Intel/x86_64 or ARM64
release. No website login or download token is required:

```sh
# Bridge: installs the daemon, pairs it, and enables its background service
curl -fsSL https://getbuild.ing/install.sh | sh

# Desktop app: installs the app for the current user
curl -fsSL https://getbuild.ing/install-desktop.sh | sh
```

The bridge goes in `~/.local/bin`. The desktop app goes in
`~/Applications/Build.app` on macOS, or `~/.local/share/build-desktop` on Linux,
with a launcher in `~/.local/bin` and an application-menu entry. Run the desktop
installer again to update. Application sign-in and bridge pairing still use
your Build account.

Both installers verify SHA-256 checksums and verify the release's Sigstore
signature when `cosign` is installed. Downloads come from the public
[`build-releases`](https://github.com/ZechCodes/build-releases/releases)
repository. The bridge follows its latest release; the desktop installer
resolves the `desktop-latest` version pointer and then downloads that specific
desktop release. This keeps the two products' versions independent.

To select a version, set `BUILD_BRIDGE_VERSION` or `BUILD_DESKTOP_VERSION` on
the shell receiving the script, for example:

```sh
curl -fsSL https://getbuild.ing/install-desktop.sh | BUILD_DESKTOP_VERSION=0.1.0 sh
```

## Build locally

Clone this repository, then run either command from its root. Both scripts build
for the current machine, download dependencies using committed lockfiles, and
print the output location. Neither installs the result or publishes a release.
An internet connection is needed for the initial dependency/tool downloads.

### Bridge (Linux or macOS)

Install current stable Rust (including Cargo), a C/C++ compiler, CMake,
pkg-config, and OpenSSL development libraries. On Debian/Ubuntu, the native
prerequisites are `build-essential cmake pkg-config libssl-dev`. On macOS,
install Xcode Command Line Tools (`xcode-select --install`), then
`brew install cmake pkg-config openssl`. Install Rust separately with rustup.

```sh
./scripts/build-bridge.sh
```

The unsigned release binary is `bridge/target/<host-target>/release/build-bridge`;
the script prints the exact path. Try that binary with `--version`. This is a
native build using your system libraries, not a portable distribution archive.
Native Windows bridge builds are not supported yet.

To use the bridge, Git and your chosen agent CLI must also be installed and
authenticated. They are runtime prerequisites, not part of the build script.

### Electron app (Linux, macOS, or Windows)

Install Node.js 22.12 or newer with npm. On macOS, also install Xcode Command
Line Tools. Linux DEB packaging also needs `libcrypt.so.1` (`libcrypt1` on
Debian/Ubuntu, `libxcrypt-compat` on Arch). Build on the OS and architecture you
want to run the app on:

```sh
node scripts/build-desktop.mjs
```

This runs `npm ci` and produces installers in `desktop/dist/`: AppImage and DEB
on Linux, DMG and ZIP on macOS, or an NSIS installer on Windows. Use
`node scripts/build-desktop.mjs --dir` for an unpacked app instead.

Local builds ignore Apple/Windows signing credentials and never publish or
notarize. macOS uses an ad-hoc signature with hardened runtime disabled for
local execution; it does not use a Developer ID certificate. These builds are
for local use. The desktop app loads the hosted web client and requires a
separately installed bridge; no SPA or backend build is needed.

### Electron releases

The `Release the desktop app` GitHub workflow runs on `desktop-vX.Y.Z` tags.
The tag must match `desktop/package.json` and its lockfile. It builds native
macOS ARM64/Intel and Linux ARM64/x86_64 apps. Mac apps are Developer ID signed
and notarized; Linux apps are unsigned. Releases include DMG/ZIP (macOS),
AppImage/DEB (Linux), installer archives, SHA-256 checksums, and a Sigstore
signature over the checksums. Assets publish to `RELEASES_REPO` (default
`ZechCodes/build-releases`), followed by the `desktop-latest/version.txt` pointer.
Desktop releases never replace the bridge's latest release.

Configure the same repository secrets used for bridge releases:
`APPLE_CERTIFICATE_P12` (base64-encoded Developer ID Application certificate),
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_APP_PASSWORD` (app-specific
password), `APPLE_TEAM_ID`, and `RELEASES_TOKEN` (write access to the releases
repository). Missing signing credentials fail the desktop release rather than
publishing unsigned assets. Local builds need none of these secrets.
Set the website's `RELEASES_REPO` environment variable to the same repository
if you override the workflow variable. Website-served installers use that
repository by default; `BUILD_RELEASES_REPO` overrides it for a single install.

## Getting started

This takes you from a fresh clone to the whole system running on your
machine: the app server, the relay and a paired bridge in containers, and the
web client you are editing served by them.

### Prerequisites

- Git.
- Rust stable (via rustup), plus the native libraries listed under
  [Build locally](#bridge-linux-or-macos): a C/C++ compiler, CMake, pkg-config
  and OpenSSL development headers.
- Node.js 22 (`spa/.nvmrc`; the desktop app needs 22.12 or newer) with npm.
- [uv](https://docs.astral.sh/uv/) for `skriftapp/` (Python 3.13 or newer,
  `skriftapp/pyproject.toml`).
- Docker or Podman with compose, for the local stack.
- Chromium or Chrome, for the web client's browser tests (on `PATH`, or
  `CHROMIUM_PATH` set to its executable). The landing check needs
  `CHROMIUM_PATH` or Playwright's own browser (see [Testing](#testing)).
- To run real agents: Claude Code or Codex CLI, installed and signed in.

### 1. Clone, with the transport beside it

The web client imports the E2EE binding from a checkout of
[`build-secure-transport`](https://github.com/ZechCodes/build-secure-transport)
**next to** this repository (`spa/package.json` resolves
`file:../../build-secure-transport/js`). Clone both into one folder:

```sh
mkdir build-dev && cd build-dev
git clone git@github.com:ZechCodes/build.git
git clone https://github.com/ZechCodes/build-secure-transport.git
npm install --no-audit --no-fund --prefix build-secure-transport/js
cd build
```

### 2. Run the local stack

[`deploy/compose.real.yml`](deploy/compose.real.yml) runs the production
topology on your machine: skriftapp with dummy sign-in, the Rust relay, and a
bridge on a sample repository with a scripted QA agent. The `qa` one-shot pairs
that bridge to the `qa@localhost` account and runs the end-to-end checks over
the DataChannels.

```sh
docker compose -f deploy/compose.real.yml up -d --build
docker compose -f deploy/compose.real.yml --profile qa run --rm qa
```

`podman compose` takes the same arguments. The qa run ends with
`QA PASS`. Then open <http://localhost:8090/app/> and sign in as
`qa@localhost` to see the paired bridge. `docker compose -f
deploy/compose.real.yml down` resets everything. There is one stack per
machine, and there are two-bridge and ICE variants: see
[`deploy/README.md`](deploy/README.md).

### 3. Work on the web client against the stack

```sh
cd spa
npm install --legacy-peer-deps
VITE_RELAY_URL=ws://localhost:18090 npm run build
cd ..
docker cp skriftapp/buildapp/static/. deploy-app-1:/app/buildapp/static/
```

Reload the page to get your build. Copying the build into the running app
container keeps its database, so the pairing survives. Rebuilding the `app`
image instead resets that database, and after that the stack needs `down` and
the two commands in step 2 again. In `spa/`, always use
`npm install --legacy-peer-deps`, never `npm ci`: the lockfile is gitignored
because the `file:` path to the transport differs per machine.

### 4. Build a bridge and pair it with your account

```sh
./scripts/build-bridge.sh
BRIDGE="$PWD/bridge/target/$(rustc -vV | sed -n 's/^host: //p')/release/build-bridge"
"$BRIDGE" --version
"$BRIDGE" pair
"$BRIDGE" install-service
```

`scripts/build-bridge.sh` only builds. It installs nothing and puts nothing on
your `PATH`, so the remaining commands use the path it printed (`$BRIDGE`).

- `"$BRIDGE" pair` prints a pairing code and waits. Enter the code in Build: on
  the welcome screen if no device is paired yet, or later under **Settings →
  Devices & keys → Add a device…**. Check that the fingerprint matches what the
  bridge printed, then approve.
- `"$BRIDGE" install-service` runs that binary in the background, as a
  `systemd --user` unit on Linux or a LaunchAgent on macOS. The service runs the
  file at that path, so a rebuild replaces what it runs at its next restart.
  To keep a stable copy instead, copy the binary somewhere first and run `pair`
  and `install-service` from the copy.

This is the same sequence `scripts/install.sh` runs for a released bridge, after
placing it in `~/.local/bin`. A locally built bridge does not update itself.
Pairing goes to getbuild.ing by default, which is invite-only.

The desktop app loads the hosted web client and requires a
separately installed bridge; no SPA or backend build is needed.

## Testing

Every gate runs under `nice -n 10`, because it shares the machine with your
bridge and your apps. **Judge each gate by its exit code**, not by its summary
line: a suite can print "passed" and still exit non-zero. CI
(`.github/workflows/ci.yml`) runs the same commands.

| Tier | Directory | Commands |
| --- | --- | --- |
| Bridge | `bridge/` | `nice -n 10 cargo test`<br>`nice -n 10 cargo clippy --all-targets -- -D warnings`<br>`nice -n 10 cargo fmt --check` |
| Web client | `spa/` | `nice -n 10 npm install --legacy-peer-deps`<br>`nice -n 10 npm run lint`<br>`nice -n 10 npm test`<br>`nice -n 10 npm run build` |
| App server | `skriftapp/` | `nice -n 10 uv run --frozen ruff check buildapp`<br>`nice -n 10 uv run --frozen pytest buildapp -q` |
| Landing page | `landing/` | `nice -n 10 npm ci`<br>`nice -n 10 npm test` |
| Landing in a browser | repo root | `skriftapp/.venv/bin/python scripts/preview-landing.py`, then in another shell `CHROMIUM_PATH=/usr/bin/chromium nice -n 10 node web/landing-check.mjs` |
| Desktop app | `desktop/` | `nice -n 10 npm ci`<br>`nice -n 10 npm test` |
| Shell scripts | repo root | `git ls-files '*.sh' \| nice -n 10 xargs shellcheck` |
| Whole system | repo root | the compose stack and `qa` run from [Getting started](#2-run-the-local-stack) |

Notes:

- **Web client.** It needs the `build-secure-transport` checkout beside the
  repository. Without it, `npm run build` cannot resolve
  `@build/secure-transport`, and hundreds of vitest cases fail. `npm test` runs
  the jsdom unit suite and the Chromium layout suite (`spa/test/browser/`);
  `npm run test:browser` runs only the latter. Run vitest from inside `spa/`,
  so that the repo's own vitest runs. `npm run lint` is the complexity gate.
- **Bridge.** The E2EE interop test checks the Rust transport against the
  Python reference in the `build-secure-transport` checkout. It only runs when
  `BUILD_SECURE_TRANSPORT_PY` points at that checkout's `python/` directory, and
  it skips (still exiting 0) when the variable is unset. From `bridge/`, as CI's
  `interop` job does:

  ```sh
  BUILD_SECURE_TRANSPORT_PY=../../build-secure-transport/python nice -n 10 cargo test --test interop_python -- --nocapture
  ```
- **Landing page.** `landing/` commits its lockfile, so `npm ci` is right there.
  `npm test` builds the Astro page, then reads it back. The preview needs that
  build and skriftapp's virtualenv (`uv sync` in `skriftapp/`).
  `web/landing-check.mjs` imports `playwright`, so install `web/`'s
  dependencies first with `npm install` in `web/`. It launches the browser named
  by `CHROMIUM_PATH` (for example `/usr/bin/chromium`). Unset, it expects
  Playwright's own download: run `npx playwright install chromium` in `web/`
  first. A browser on `PATH` alone is not enough. The preview listens on port
  4173. If that port is taken, start it with `--port <n>` and set
  `LANDING_URL=http://127.0.0.1:<n>`.
- **App server.** `uv run` creates `skriftapp/.venv` on first use.

## Contributing

- **Branch from `main`, one task per branch.** Branches are named
  `build/<short-slug>`.
- **Nothing merges without review.** Every change is reviewed before it lands
  on `main`. A merge commit names the task: `Merge #128: bridge liveness under
  load`.
- **Test first.** Write the failing test, then the code. Commit as you go, not
  in one lump at the end.
- **Keep functions small.** Each tier caps function complexity: ruff `C901` 10
  in `skriftapp/`, clippy `cognitive_complexity` 15 in `bridge/`
  (`bridge/clippy.toml`), eslint `complexity` 10 in `spa/`
  (`spa/eslint.config.js`). Functions that were already over the cap carry a
  ratchet annotation, and the counts are pinned by
  `bridge/tests/complexity_ratchet.rs` and `spa/test/complexityRatchet.test.js`.
  **No change adds to a ratchet list.** A new function over the cap is split,
  not annotated. Retiring one is welcome, in its own commit.
- **Scan before every commit.** Run semgrep and gitleaks over your change:

  ```sh
  semgrep scan --config p/security-audit --config p/secrets --error --metrics off <changed files>
  gitleaks git --no-banner --redact --log-opts='main..HEAD' .
  ```

  semgrep only scans files git tracks, so `git add` new files first. Features
  with a security surface have a checklist in `planning/v2/*Security
  Checklist.md`, and it must be fully met.
- **Commit messages** say what is now true, in one sentence, with the task
  number at the end: `Task list reads are single-flight under push churn
  (#119)`. An area prefix is common when the change stays in one tier:
  `bridge: keep liveness off the app lock; agents in their own slice (#128)`,
  `docs: …`, `web: …`. The body explains why. Version bumps are
  `chore(bridge): X.Y.Z`.
- **Specs** live in [`planning/v2/`](planning/v2/). They record intent and can
  lag the code. **Where a spec and the code disagree, the code is the source of
  truth.** Build what was asked, then amend the spec.
- **Agents** follow [`AGENTS.md`](AGENTS.md) (which `CLAUDE.md` imports). It
  carries these rules plus the gate recipes.

## Develop

```bash
cd bridge && nice -n 10 cargo test && nice -n 10 cargo clippy --all-targets -- -D warnings && nice -n 10 cargo fmt --check
cd skriftapp && uv run --frozen ruff check buildapp && uv run --frozen pytest buildapp
cd spa && nice -n 10 npm run lint && nice -n 10 npm test && nice -n 10 npm run build
cd desktop && nice -n 10 npm test && nice -n 10 npm run pack
```

The bridge's ICE agent is tuned by three variables (strict P2P transport spec,
rule 8): `BRIDGE_ICE_POLICY` (`all`, the default, or `direct-only` — strip the
browser's TURN servers and refuse every relay candidate it sends, trickled or
carried in the offer), `BRIDGE_ICE_RELAY_MIN_WAIT_MS`
(how long a TURN pair waits before it may be accepted so a slower direct pair can
win; default `1500`, `0` for no wait) and `BRIDGE_ICE_INTERFACES` (a comma list of
interfaces to gather host candidates on, e.g. `tailscale0,eth0`; unset means every
non-loopback interface, IPv4 and — where the machine has an address of its own —
IPv6). Every `BRIDGE_*` variable is listed in `bridge/src/main.rs`.

Full local stack (app + relay + bridge + scripted QA) via podman compose:
see [`deploy/README.md`](deploy/README.md). Production deploy:
[`deploy/k8s/CUTOVER.md`](deploy/k8s/CUTOVER.md).

## License

Copyright (C) 2026 Zech Zimmerman

Build is licensed under the [GNU Affero General Public License v3.0
only](LICENSE) (`AGPL-3.0-only`). You are free to use, modify and self-host it.
If you run a modified version as a network service, you must offer its source
to the service's users under the same license. Commercial licensing is
available from the maintainer. The E2EE transport is published separately for
auditability, under its own license.
