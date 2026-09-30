# Running Build v2

These manifests deploy the maintainers' instance; adapt the namespace, context and hostnames for your own.

> **Production (Kubernetes):** see [`k8s/`](k8s/) — kustomize manifests for the
> `8ly` namespace (app + Rust relay + Postgres), `bootstrap-secrets.sh`, and the
> [`k8s/CUTOVER.md`](k8s/CUTOVER.md) runbook.

## Local stack (compose)

`compose.real.yml` runs the production topology locally — **skriftapp**
(auth + device registry + gateway tokens + the SPA), the **Rust relay**
(single broker: `/ws/device` + `/ws/client`), and a **bridge** with the
deterministic QA agent. No gateway, no Redis: browsers talk straight to the
relay with an api-minted gateway token, and the relay validates everything
against the app over `/internal/*` with `X-Internal-Secret`.

The relay is a rendezvous, not a data plane: a client opens it to mint its
sessions and negotiate, then closes it and runs everything over the WebRTC
DataChannels. That works inside compose with no Cloudflare account — see
[ICE servers](#ice-servers-the-webrtc-upgrade) below.

```bash
podman compose -f deploy/compose.real.yml up -d --build
podman compose -f deploy/compose.real.yml --profile qa run --rm qa
open http://localhost:8090/app/        # dummy login (any email)
podman compose -f deploy/compose.real.yml down
```

| Service | What it is | Host port |
|---|---|---|
| `app`   | skriftapp: dummy auth (dev), devices api, gateway tokens, SPA | 8090 |
| `relay` | ciphertext-only broker (`/ws/client`, `/ws/device`, `/health`) | 18090 |
| `bridge`| device daemon on a sample `/repo`, `BRIDGE_QA_AGENT=1` | — |
| `qa`    | one-shot: the harness unit tests, then pairs the bridge, then `e2e.mjs` + `qa.mjs` over the DataChannels | — |

Pairing is the real device-initiated flow: the bridge registers *pending* with
a deterministic code (`BRIDGE_PAIRING_CODE`, default `COMPOSE-PAIR`) and the
qa one-shot — or you, in the SPA under Settings → Devices — approves it. All
state (app sqlite, bridge identity) is container-lifetime: `down` + `up`
resets the world consistently; `restart` keeps it.

No secrets to provision: dev defaults are baked into the compose file and can
be overridden with `BUILD_SECRET_KEY`, `BUILD_INTERNAL_API_SECRET`,
`BUILD_PAIRING_CODE`.

**One stack per machine, not one per checkout.** Compose names the project after
this directory, so every worktree of this repo drives the same `deploy-app-1`,
`deploy-relay-1` and `deploy-bridge-1` containers on the same host ports — two
people (or two agents) bringing the stack up from different checkouts rebuild
and re-pair on top of each other, and the app's sqlite is container-lifetime, so
whoever recreates it takes the other's account with it. Say whose stack it is
with `-p`:

```bash
docker compose -p device-presence -f deploy/compose.real.yml up -d --build
```

That separates the containers, not the host ports — 8090 and 18090 are written
into the compose file — so a second stack still needs those two lines edited
before it can come up beside the first.

### Two bridges on one account

Anything about *which* device — two machines' work in one rail, the device tag
on a project name two machines share, the picker's filter, one bridge going
while the other keeps working — needs a second bridge, which
[`compose.two-bridges.yml`](compose.two-bridges.yml) adds: it names the base
bridge **Laptop** and stands up a **Desktop** with its own identity, repo and
pairing code (`COMPOSE-PAIR-2`).

```bash
docker compose -f deploy/compose.real.yml -f deploy/compose.two-bridges.yml up -d --build
docker compose -f deploy/compose.real.yml -f deploy/compose.two-bridges.yml \
  --profile qa run --rm qa node pair.mjs
API_URL=http://localhost:8090 PAIRING_CODE=COMPOSE-PAIR-2 node web/pair-another.mjs
```

`pair.mjs` stops as soon as the account owns a device, so the second one is
approved by [`../web/pair-another.mjs`](../web/pair-another.mjs) — the same
lookup→approve flow without that guard.

Each machine gets its own rendezvous and its own peer connection, so pin the one
under test with `PREFER_DEVICE_ID` (`GET /api/devices` lists both ids); without
it the harness takes the first device the api reports online. `web/wire-check.mjs`
is the by-hand pass over this stack:

```bash
cd web && PREFER_DEVICE_ID=<id> node wire-check.mjs
```

Two things bite when the stack is not on the default ports. The app image bakes
the SPA, so `VITE_RELAY_URL` is a build arg — a moved relay port needs
`up -d --build app`. And `skriftapp/app.dev.yaml`'s CSP `connect-src` hard-codes
`ws://localhost:18090`, so a browser pass against a moved relay needs that
widened or the socket never opens. Recreating `app` also resets its sqlite while
the bridges keep their identities, so both bridge containers must be
`--force-recreate`d before pairing again.

## ICE servers (the WebRTC upgrade)

A browser reaches its bridge over WebRTC DataChannels and nothing else, falling
back to Cloudflare TURN when neither peer can hole-punch. The browser fetches the
server list from the api (`POST /api/rtc/ice-servers`, session-cookie
authenticated) and forwards it to the bridge inside the sealed session, so the
bridge needs no Cloudflare access.

| Env key | Where it comes from | What it is |
|---|---|---|
| `CF_TURN_KEY_ID` | `build-app` Secret | Cloudflare TURN key the api mints per-user credentials from; their lifetime is `TTL_SECONDS` in `skriftapp/buildapp/ice_servers.py` |
| `CF_TURN_KEY_API_TOKEN` | `build-app` Secret | That key's API token. Never returned to a browser or a bridge |

Both are optional (`optional: true` in [`k8s/app.yaml`](k8s/app.yaml);
[`k8s/bootstrap-secrets.sh`](k8s/bootstrap-secrets.sh) patches them in when
they are exported and reports their absence instead of failing). With
neither set — which is how `compose.real.yml` runs — the route answers a
STUN-only list, and that is all the local stack needs: every container is on one
compose network, so the pair the browser or the qa harness nominates with the
bridge is **host to host**. That STUN-only list is
`stun:stun.cloudflare.com:3478` — unauthenticated, no account, and
unreachable-tolerant: with it down or the machine offline, host candidates still
carry localhost sessions. Unreachable-tolerant is not unused, though: the bridge
does send a binding request to it and trickles the server-reflexive candidate it
gets back (the public address of the machine the stack runs on). Nothing
nominates that candidate here — plan for the outbound UDP, not against the
pairing, if the deployment is air-gapped or egress-filtered.

A deployment without the key is **not** a deployment that degrades gracefully:
there is no relay underneath the peer connection any more. A browser and a
bridge that can reach each other neither directly nor through TURN show that
machine as blocked, with the reason and a Retry. Over the open internet — the
two behind different NATs — that is what the TURN key is for.

TURN egress is billed, so it has a monthly check in [`OPS.md`](OPS.md).

### LAN / Tailscale

When the browser and the bridge share a network — an office LAN, a Tailnet, a
laptop reaching its own machine — set `BRIDGE_ICE_POLICY=direct-only` on the
bridge. It then strips every `turn:`/`turns:` url out of the list the browser
offers and refuses every relay candidate the browser sends — trickled, or
carried inside the offer — so this bridge allocates no TURN and pairs with none
that it can name. STUN is kept: it is free, and it is how a peer learns the
address it puts in a host candidate. Nothing changes in the browser — it fetches
and forwards the same minted list either way — and nothing changes on the api.

One case the bridge cannot close from its end: the browser is the controlling
agent and still has its own TURN servers, so a browser behind a symmetric NAT
can allocate one and check from it. A check from an address no candidate named
arrives as a *peer-reflexive* candidate the ICE agent creates for itself — the
bridge never sees it as a relay candidate, and cannot refuse it — so that pair
can still carry, and `/admin/transport` will read it as direct. Chrome's own
prioritisation puts a direct pair first where one exists; where none does,
`direct-only` prevents the bridge's half of the egress, not the browser's.

Two knobs go with it. `BRIDGE_ICE_INTERFACES` (e.g. `tailscale0,eth0`) binds
only the named interfaces, which is how a bridge is kept off a network it
should not be reachable on; a name no address answers to refuses the offer
rather than quietly gathering everywhere, so a typo is loud. And
`BRIDGE_ICE_RELAY_MIN_WAIT_MS` (default 1500, `0` to disable) is how long a
TURN pair waits before this agent may accept it — the "prefer direct" margin
on a hosted bridge, irrelevant under `direct-only`, where there is no TURN
pair to wait for.

## Releasing the bridge

The bridge is the one piece that runs on someone else's machine, so it ships as
a GitHub release rather than an image. `.github/workflows/release.yml` builds it
on a `bridge-vX.Y.Z` tag and publishes to a **separate public repo**, so that
downloading a binary never means being handed this repository.

### Prerequisites (one-time, and none of them are created by the pipeline)

| What | Where | Why |
|---|---|---|
| The releases repo, **public, with a default branch and at least one commit** | default `ZechCodes/build-releases` | `gh release create` needs a commit to hang the tag on. The workflow will not create it |
| Secret `RELEASES_TOKEN` | build → Settings → Secrets | Fine-grained PAT with `contents: write` on the releases repo, and nothing else. The only secret the pipeline requires |
| Variable `RELEASES_REPO` | build → Settings → Variables | Optional. Overrides the default owner/name. The api reads the same name from its own environment, and install.sh from `BUILD_RELEASES_REPO` — keep the three in step |
| Secrets `APPLE_CERTIFICATE_P12`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_ID`, `APPLE_APP_PASSWORD` | build → Settings → Secrets | Optional. All five present ⇒ the macOS binaries are Developer-ID signed and notarized. Any one missing ⇒ a warning is logged, signing is skipped, and the unsigned builds publish anyway |

Until those Apple credentials exist the macOS downloads are unsigned, and a
macOS user gets one Gatekeeper refusal the first time they run the binary —
"Apple could not verify build-bridge is free of malware". They clear it in
System Settings → Privacy & Security → *Open Anyway*, once. Linux is unaffected.

### The procedure

```bash
# 1. bump the one home of the version
$EDITOR bridge/Cargo.toml         # version = "X.Y.Z"
(cd bridge && cargo check)        # refreshes Cargo.lock
git commit -am 'chore(bridge): X.Y.Z'
# 2. merge to main, then tag the commit that is going out
git tag bridge-vX.Y.Z && git push origin bridge-vX.Y.Z
```

The tag is the trigger and the version check is the first job: if `X.Y.Z` is not
what `bridge/Cargo.toml` declares, nothing is built. Each binary is then made to
print its own `--version` and compared to the tag again before it is packaged.

Each macOS signing leg waits for Apple's notarization for at most 90 minutes
(`scripts/notarize.sh`). When Apple is slower, the leg fails with its
submission id in the job summary, and nothing is published. Resume it
without re-signing once Apple finishes, by dispatching the workflow on the
same tag with the id or ids that leg printed:

```bash
gh workflow run release.yml --ref bridge-vX.Y.Z \
  -f notarization-id-macos-arm64=<id> -f notarization-id-macos-x86_64=<id>
```

A leg given an id polls that submission and does not resubmit. It still fails
when the ticket does not list the CDHash of the binary it just signed, which
happens when the rebuilt binary differs from the one submitted. Leave that
leg's input empty to submit afresh.

### What a release contains

Asset names carry no version, so `releases/latest/download/<name>` is a stable
URL that the api, the web client and install.sh can all hardcode.

| Asset | What it is |
|---|---|
| `build-bridge-macos-arm64.tar.gz` | one entry, `build-bridge`, mode 0755, at the archive root |
| `build-bridge-macos-x86_64.tar.gz` | same |
| `build-bridge-linux-x86_64.tar.gz` | same |
| `build-bridge-linux-aarch64.tar.gz` | same |
| `SHA256SUMS` | `sha256sum` format, one line per tarball plus `install.sh` |
| `SHA256SUMS.sigstore.json` | keyless Sigstore bundle over `SHA256SUMS` |
| `install.sh` | `scripts/install.sh` verbatim at the tag |

All four are built on native runners — no cross toolchains — which is also what
sets the floor: **Linux needs glibc 2.35 or newer** (Ubuntu 22.04, Debian 12,
Fedora 36) and **macOS 13 or newer**. libgit2 and OpenSSL are compiled into the
binary (`release-portable` in `bridge/Cargo.toml`), so glibc is the only shared
library a download asks of its host.

There is no signing key to rotate or lose: cosign signs with the OIDC token
GitHub mints for the workflow, so the certificate's identity *is*
`…/build-web/.github/workflows/release.yml@refs/tags/bridge-vX.Y.Z` (or
`…/build/…` once the repository is renamed to `build`; both names are
accepted). That identity is what install.sh and the bridge's updater pin,
which is why the workflow file's path may not move without updating them. The
release refuses to build when release.yml and install.sh disagree: the
`version` job greps install.sh for the identity release.yml carries in its own
`COSIGN_IDENTITY_REGEXP`.

### What a user does

```bash
curl -fsSL https://getbuild.ing/install.sh | sh
```

`GET /install.sh` serves the bridge script packaged in the website image, with
the deployment's `RELEASES_REPO` as its default source. The endpoint and downloads
are public and need no login or token. The script maps `uname` to a platform key,
downloads the release tarball from the public releases repository
with `SHA256SUMS` and the bundle, verifies the digest (always) and the signature
(whenever `cosign` is on PATH), installs the binary, then runs `build-bridge
pair` — which prints a code and blocks until the human approves that device in
Build — and `build-bridge install-service`, a launchd LaunchAgent on macOS or a
`systemd --user` unit on Linux.

| Env | Default | Effect |
|---|---|---|
| `BUILD_BRIDGE_VERSION` | `latest` | `X.Y.Z` installs that release instead |
| `BUILD_BRIDGE_INSTALL_DIR` | `$HOME/.local/bin` | where the binary lands |
| `BUILD_RELEASES_REPO` | `ZechCodes/build-releases` | where to download from |
| `BUILD_BRIDGE_SKIP_SERVICE` | unset | `1` stops after the binary — no pairing, no service |

Verifying by hand is the same two commands the script runs:

```bash
sha256sum --check --ignore-missing SHA256SUMS
cosign verify-blob \
  --bundle SHA256SUMS.sigstore.json \
  --certificate-identity-regexp '^https://github\.com/ZechCodes/(build-web|build)/\.github/workflows/release\.yml@refs/tags/bridge-v' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  SHA256SUMS
```

### Smoke test after a release

```bash
BUILD_BRIDGE_SKIP_SERVICE=1 sh <(curl -fsSL https://getbuild.ing/install.sh)
~/.local/bin/build-bridge --version    # build-bridge X.Y.Z
```

Then pair and install the service for real on one macOS and one Linux machine —
the workflow cannot prove either, and a re-run of `publish` is safe (the release
is created only if missing and every asset uploads with `--clobber`).

## Releasing and installing the desktop app

`.github/workflows/release-desktop.yml` runs on `desktop-vX.Y.Z` tags matching
`desktop/package.json` and `desktop/package-lock.json`. It builds Linux x86_64
and ARM64 AppImage/DEB packages and macOS Intel/Apple Silicon DMG/ZIP packages.
Mac signing and notarization are required; use the same Apple secrets and
`RELEASES_TOKEN` as the bridge workflow. Missing credentials fail the release.

The public release also contains stable installer archive names:
`build-desktop-macos-{arm64,x86_64}.zip` and
`build-desktop-linux-{x86_64,aarch64}.tar.gz`, plus `SHA256SUMS` and
`SHA256SUMS.sigstore.json`. The checksum signature pins
`…/build-web/.github/workflows/release-desktop.yml@refs/tags/desktop-vX.Y.Z`
(or `…/build/…` after the rename).

After publishing the versioned assets, the workflow updates `version.txt` in
the `desktop-latest` channel release. Both desktop releases and the channel
are marked `--latest=false` so bridge downloads continue to use GitHub's latest
release. Installers read the desktop pointer once and pin the subsequent
downloads to that version.

```sh
curl -fsSL https://getbuild.ing/install-desktop.sh | sh
```

The website serves this script from its image, with the same `RELEASES_REPO`
default as all other download URLs. No `GITHUB_RELEASES_TOKEN` is needed by the
website. Keep its `RELEASES_REPO` environment value aligned with the workflow
variable. `BUILD_RELEASES_REPO` and `BUILD_DESKTOP_VERSION` override the source
and version for an individual install.

The installer uses user-owned locations, never requests sudo, and does not
launch the app. On Linux, install the usual Electron desktop runtime libraries
provided by your distribution (GTK, NSS, X11/Wayland, and audio libraries).
Run the installer again to update. Publishing a release and deploying the
website image are separate operations: both must complete before the new
desktop one-liner works on the production site.

## Known-stale harnesses

`web/qa-reconnect.mjs` still speaks the pre-auth gateway protocol (no
`authenticate` first frame) and still calls `openSession` with its pre-peer
signature; it needs updating before it runs against this stack.
`web/e2e.mjs`, `web/qa.mjs`, `web/wire-check.mjs`, `web/pair.mjs`,
`web/pair-another.mjs` and `web/skrift-flow.mjs` are current.

The relay-carried terminal clients — `web/terminal.mjs`, `web/terminal.html` and
their drivers `term-verify.mjs`, `term-browser.mjs`, `term-perf.mjs`,
`term-size-check.mjs` — were **deleted** rather than updated. A terminal stream
over a relay socket is the one thing the strict-P2P rules make impossible, so
there was nothing to port; the terminal now rides the `term` DataChannel, which
`web/qa.mjs` exercises and `spa/` is the real client for.
