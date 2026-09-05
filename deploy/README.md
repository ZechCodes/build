# Running Build v2

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
| `qa`    | one-shot: pairs the bridge, then `e2e.mjs` + `qa.mjs` (16 checks) | — |

Pairing is the real device-initiated flow: the bridge registers *pending* with
a deterministic code (`BRIDGE_PAIRING_CODE`, default `COMPOSE-PAIR`) and the
qa one-shot — or you, in the SPA under Settings → Devices — approves it. All
state (app sqlite, bridge identity) is container-lifetime: `down` + `up`
resets the world consistently; `restart` keeps it.

No secrets to provision: dev defaults are baked into the compose file and can
be overridden with `BUILD_SECRET_KEY`, `BUILD_INTERNAL_API_SECRET`,
`BUILD_PAIRING_CODE`.

## ICE servers (the WebRTC upgrade)

Once a browser session is live over the relay it upgrades to a direct WebRTC
DataChannel to the bridge, and falls back to Cloudflare TURN when neither peer
can hole-punch. The browser fetches the server list from the api
(`POST /api/rtc/ice-servers`, session-cookie authenticated) and forwards it to
the bridge inside the sealed session, so the bridge needs no Cloudflare access.

| Env key | Where it comes from | What it is |
|---|---|---|
| `CF_TURN_KEY_ID` | `build-app` Secret | Cloudflare TURN key the api mints per-user credentials from; their lifetime is `TTL_SECONDS` in `skriftapp/buildapp/ice_servers.py` |
| `CF_TURN_KEY_API_TOKEN` | `build-app` Secret | That key's API token. Never returned to a browser or a bridge |

Both are optional (`optional: true` in [`k8s/app.yaml`](k8s/app.yaml);
[`k8s/bootstrap-secrets.sh`](k8s/bootstrap-secrets.sh) patches them in when
they are exported and reports their absence instead of failing). With
neither set — which is how `compose.real.yml` runs — the route answers a
STUN-only list and direct host candidates carry localhost sessions, so the
local stack needs no Cloudflare account. That STUN-only list is
`stun:stun.cloudflare.com:3478` — unauthenticated, no account, and
unreachable-tolerant: with it down or the machine offline, host candidates
still carry localhost sessions. A deployment without the key is supported
too: peers that cannot hole-punch simply keep working over the relay.

TURN egress is billed, so it has a monthly check in [`OPS.md`](OPS.md).

## Releasing the bridge

The bridge is the one piece that runs on someone else's machine, so it ships as
a GitHub release rather than an image. `.github/workflows/release.yml` builds it
on a `bridge-vX.Y.Z` tag and publishes to **this repository's own Releases**
with the `GITHUB_TOKEN` Actions mints for the run — no PAT, no second repo.

This repository is private for the alpha and goes public at launch, which is
the one fact the download path bends around: while it is private the assets are
not anonymously fetchable, so the api streams them to alpha members
(`GET /app/downloads/…`, see [`../skriftapp/README.md`](../skriftapp/README.md));
once it is public the same routes 302 to the release asset and the api needs no
credential at all.

### Prerequisites (one-time, and none of them are created by the pipeline)

| What | Where | Why |
|---|---|---|
| Secret `GITHUB_RELEASES_TOKEN` | build-app Secret, read by the api — **not** an Actions secret | Fine-grained PAT, `Contents: read` on `ZechCodes/build-web` and nothing else. It is what lets the api fetch a release asset out of a private repo. Optional (`optional: true` in [`k8s/app.yaml`](k8s/app.yaml); [`k8s/bootstrap-secrets.sh`](k8s/bootstrap-secrets.sh) patches it in when exported). **Remove it at launch** — with no token the api redirects to the public asset instead |
| Secrets `APPLE_CERTIFICATE_P12`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_ID`, `APPLE_APP_PASSWORD` | build-web → Settings → Secrets | Optional. All five present ⇒ the macOS binaries are Developer-ID signed and notarized. Any one missing ⇒ a warning is logged, signing is skipped, and the unsigned builds publish anyway |

The release pipeline itself needs no secret: `GITHUB_TOKEN` is issued per run,
and only the `publish` job holds `contents: write`.

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

### What a release contains

Asset names carry no version, so `releases/latest` is the one release the api
ever asks for, and the six names below are what it looks for inside it.

| Asset | What it is |
|---|---|
| `build-bridge-macos-arm64.tar.gz` | one entry, `build-bridge`, mode 0755, at the archive root |
| `build-bridge-macos-x86_64.tar.gz` | same |
| `build-bridge-linux-x86_64.tar.gz` | same |
| `build-bridge-linux-aarch64.tar.gz` | same |
| `SHA256SUMS` | `sha256sum` format, one line per tarball |
| `SHA256SUMS.sigstore.json` | keyless Sigstore bundle over `SHA256SUMS` |

`install.sh` is not among them: the api serves it from the app image (`COPY
scripts/install.sh` in the Containerfile) with a download token substituted in,
so there is no copy on a release to drift from the one in this tree.

All four builds are made on native runners — no cross toolchains — which is also
what sets the floor: **Linux needs glibc 2.35 or newer** (Ubuntu 22.04, Debian
12, Fedora 36) and **macOS 13 or newer**. libgit2 and OpenSSL are compiled into
the binary (`release-portable` in `bridge/Cargo.toml`), so glibc is the only
shared library a download asks of its host.

There is no signing key to rotate or lose: cosign signs with the OIDC token
GitHub mints for the workflow, so the certificate's identity *is*
`…/build-web/.github/workflows/release.yml@refs/tags/bridge-vX.Y.Z`. That string
is what install.sh pins, which is why the workflow file's path may not move
without updating both. The release refuses to build when they disagree: the
`version` job greps install.sh for the identity release.yml carries in its own
`COSIGN_IDENTITY_REGEXP`.

### What a user does

They copy their install line from Build — Settings → Downloads — and run it:

```bash
curl -fsSL "https://getbuild.ing/install.sh?t=dl_…" | sh
```

The `t=` is a download token: minted for that member when the page renders,
good for ten minutes, and spent by one binary download. A second install needs
a freshly copied line. The api serves `/install.sh` with that token and its own
origin substituted into the script, so nothing has to be typed or exported.

install.sh maps `uname` to a platform key and then asks the api for three
things in this order — `SHA256SUMS`, `SHA256SUMS.sigstore.json`, and the
tarball last, because the tarball is the request that spends the token. It
verifies the digest (always) and the signature (whenever `cosign` is on PATH),
installs the binary, then runs `build-bridge pair` — which prints a code and
blocks until the human approves that device in Build — and `build-bridge
install-service`, a launchd LaunchAgent on macOS or a `systemd --user` unit on
Linux.

Two refusals a user can act on, and both name the same fix: a script with no
token in it stops before it downloads anything (exit 2), and a `401` from the
api — an expired, already-spent, or revoked line — ends the install with
nothing written (exit 1). Copy a fresh line from Build.

| Env | Default | Effect |
|---|---|---|
| `BUILD_BRIDGE_INSTALL_DIR` | `$HOME/.local/bin` | where the binary lands |
| `BUILD_BRIDGE_SKIP_SERVICE` | unset | `1` stops after the binary — no pairing, no service |

Verifying by hand is the same two commands the script runs:

```bash
sha256sum --check --ignore-missing SHA256SUMS
cosign verify-blob \
  --bundle SHA256SUMS.sigstore.json \
  --certificate-identity-regexp '^https://github\.com/ZechCodes/build-web/\.github/workflows/release\.yml@refs/tags/bridge-v' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  SHA256SUMS
```

### Smoke test after a release

Copy a fresh install line from Settings → Downloads and run it with the service
step skipped:

```bash
BUILD_BRIDGE_SKIP_SERVICE=1 sh -c "$(curl -fsSL 'https://getbuild.ing/install.sh?t=dl_…')"
~/.local/bin/build-bridge --version    # build-bridge X.Y.Z
```

Then pair and install the service for real on one macOS and one Linux machine —
the workflow cannot prove either, and a re-run of `publish` is safe (the release
is created only if missing and every asset uploads with `--clobber`).

## Known-stale harnesses

`web/qa-reconnect.mjs`, `web/term-verify.mjs`, `web/term-browser.mjs` still
speak the pre-auth gateway protocol (no `authenticate` first frame) and need
updating before they run against this stack. `web/e2e.mjs`, `web/qa.mjs`,
`web/skrift-flow.mjs`, and `web/pair.mjs` are current.
