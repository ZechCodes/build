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
on a `bridge-vX.Y.Z` tag and publishes to a **separate public repo**, so that
downloading a binary never means being handed this repository.

### Prerequisites (one-time, and none of them are created by the pipeline)

| What | Where | Why |
|---|---|---|
| The releases repo, **public, with a default branch and at least one commit** | default `ZechCodes/build-releases` | `gh release create` needs a commit to hang the tag on. The workflow will not create it |
| Secret `RELEASES_TOKEN` | build-web → Settings → Secrets | Fine-grained PAT with `contents: write` on the releases repo, and nothing else. The only secret the pipeline requires |
| Variable `RELEASES_REPO` | build-web → Settings → Variables | Optional. Overrides the default owner/name. The api reads the same name from its own environment, and install.sh from `BUILD_RELEASES_REPO` — keep the three in step |
| Secrets `APPLE_CERTIFICATE_P12`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_ID`, `APPLE_APP_PASSWORD` | build-web → Settings → Secrets | Optional. All five present ⇒ the macOS binaries are Developer-ID signed and notarized. Any one missing ⇒ a warning is logged, signing is skipped, and the unsigned builds publish anyway |

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
`…/build-web/.github/workflows/release.yml@refs/tags/bridge-vX.Y.Z`. That string
is what install.sh pins, which is why the workflow file's path may not move
without updating both. The release refuses to build when they disagree: the
`version` job greps install.sh for the identity release.yml carries in its own
`COSIGN_IDENTITY_REGEXP`.

### What a user does

```bash
curl -fsSL https://getbuild.ing/install.sh | sh
```

`GET /install.sh` on the app is a 302 to
`https://github.com/<RELEASES_REPO>/releases/latest/download/install.sh`, so the
one-liner works only once a release exists — before the first one it lands on a
GitHub 404. The script maps `uname` to a platform key, downloads that tarball
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
  --certificate-identity-regexp '^https://github\.com/ZechCodes/build-web/\.github/workflows/release\.yml@refs/tags/bridge-v' \
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

## Known-stale harnesses

`web/qa-reconnect.mjs`, `web/term-verify.mjs`, `web/term-browser.mjs` still
speak the pre-auth gateway protocol (no `authenticate` first frame) and need
updating before they run against this stack. `web/e2e.mjs`, `web/qa.mjs`,
`web/skrift-flow.mjs`, and `web/pair.mjs` are current.
