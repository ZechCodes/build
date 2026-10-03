# Contributing

Issues and pull requests are welcome. This file is the home for everything a
contributor needs: setting up, building, testing, and the working rules.
[`ARCHITECTURE.md`](ARCHITECTURE.md) explains how the bridge and the web client
are built, and [`AGENTS.md`](AGENTS.md) carries the same rules plus the full
gate recipes for agents. Security problems are reported privately; see
[SECURITY.md](SECURITY.md).

## Contributor License Agreement

Pull requests need a signed [Contributor License Agreement](CLA.md). A bot
asks on your first pull request; you sign once by replying with the comment it
gives you, and the signature covers your later pull requests. Signatures are
recorded in `signatures/cla.json` on the `cla-signatures` branch.

The project is published under the [license](LICENSE) (AGPL-3.0-only).

## Before you write code

Before implementing a change or submitting a pull request, open a
[GitHub issue](https://github.com/ZechCodes/Build/issues) describing the problem
or your proposal, and discuss it with the maintainers. Reference the issue in
your pull request so reviewers can follow the discussion.

Small typo fixes and obvious one-line bugs can go straight to a pull request.

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
- To run real agents: Claude Code, Codex CLI or Pi, installed and signed in.

### 1. Clone, with the transport beside it

The web client imports the E2EE binding from a checkout of
[`build-secure-transport`](https://github.com/ZechCodes/build-secure-transport)
**next to** this repository (`spa/package.json` resolves
`file:../../build-secure-transport/js`). Clone both into one folder:

```sh
mkdir build-dev && cd build-dev
git clone https://github.com/ZechCodes/Build.git
git clone https://github.com/ZechCodes/build-secure-transport.git
npm install --no-audit --no-fund --prefix build-secure-transport/js
cd Build
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

The desktop release workflow and its signing secrets are in
[`docs/releasing.md`](docs/releasing.md).

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

## Working rules

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
rule 8): `BRIDGE_ICE_POLICY` (`all`, the default, or `direct-only`, which strips the
browser's TURN servers and refuses every relay candidate it sends, trickled or
carried in the offer), `BRIDGE_ICE_RELAY_MIN_WAIT_MS`
(how long a TURN pair waits before it may be accepted so a slower direct pair can
win; default `1500`, `0` for no wait) and `BRIDGE_ICE_INTERFACES` (a comma list of
interfaces to gather host candidates on, e.g. `tailscale0,eth0`; unset means every
non-loopback interface, IPv4 and, where the machine has an address of its own,
IPv6). Every `BRIDGE_*` variable is listed in `bridge/src/main.rs`.

Full local stack (app + relay + bridge + scripted QA) via podman compose:
see [`deploy/README.md`](deploy/README.md). Production deploy:
[`deploy/k8s/CUTOVER.md`](deploy/k8s/CUTOVER.md).
