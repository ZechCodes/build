# Running Build v2 over the real relay

> **Production (Kubernetes):** see [`k8s/`](k8s/) — kustomize manifests for the
> `8ly` namespace (app + Rust relay + Postgres), `bootstrap-secrets.sh`, and the
> [`k8s/CUTOVER.md`](k8s/CUTOVER.md) runbook. This compose stack below is the
> local/dev topology (it still uses the v1-era build-relay + gateway shim).

This stack runs the **real `build-relay`** (Postgres + Redis + the relay), the
Rust bridge, the client gateway (the web-backend's relay-facing shim), and the
web client. Non-default host ports so it coexists with other relay deployments:

| Service | Host port |
|---|---|
| relay (`/ws/device`, `/internal`, `/health`) | 18081 |
| client gateway (browser WS) | 18090 |
| web client | 18080 |
| postgres | 15432 |
| redis | 16379 |

## Why a gateway?

`build-relay` is **device-facing only** — it exposes `/ws/device` and an
`/internal/device/{id}/send` API, and publishes device→app events to the Redis
stream `relay:device-events`. The browser never talks to the relay directly; a
web backend mediates. `gateway/` is a minimal stand-in for that backend: browser
WS in, `/internal/send` + Redis events out.

## One-time setup

```bash
# 1. Build the real relay image from your build-relay checkout
podman build -t build-relay:local /path/to/build-relay

# 2. Build the bridge binary (used to mint a device identity)
( cd bridge && cargo build --release --bin build-bridge )

# 3. Provision a device identity → deploy/secrets/{seed.sql,bridge.env,gateway.env}
python3 deploy/provision.py

# 4. Generate the postgres password (git-ignored; no credentials in compose)
echo "BUILD_POSTGRES_PASSWORD=$(openssl rand -hex 24)" > deploy/secrets/postgres.env
```

`provision.py` mints a UUID device id + Ed25519 identity key + X25519 transport
key. The padded Ed25519 public key is seeded into `build.devices` (approved) so
the relay authenticates the bridge; the private keys go to the bridge; the
transport public key + device id go to the gateway. Secrets live in
`deploy/secrets/` (git-ignored).

## Up / verify / down

```bash
podman compose --env-file deploy/secrets/postgres.env -f deploy/compose.real.yml up -d --build
curl -s localhost:18081/health           # {"connected_devices": 1}
( cd web && RELAY_URL=ws://localhost:18090 node qa.mjs )            # full app RPC
( cd web && RELAY_URL=ws://localhost:18090 node qa-reconnect.mjs )  # reconnect
podman compose --env-file deploy/secrets/postgres.env -f deploy/compose.real.yml down
```

## Reconnect verification

A deterministic simulated agent streams N ordered chunks (`chunk-NNNNNN`) into the
bridge's **authoritative, seq-numbered log** (keyed by stream id, not by session).
The client resumes via `stream.events {since}`; we prove its reconstructed output
reconverges *exactly* (matching sha256, contiguous seqs, no gaps/dupes):

```bash
cd web
RELAY_URL=ws://localhost:18090 node qa-reconnect.mjs   # scenarios A + B
```

- **A — client disconnect mid-stream**: drop the WS partway, reconnect with a new
  E2EE session, resume from the last seq → full convergence.
- **B — reconnect while away (load)**: leave immediately; the stream finishes at
  the bridge; reconnect and replay the whole backlog in **bounded batches**.
- **C — bridge-side reconnect** (the old system's weak spot): bounce the relay
  mid-stream so the device drops and re-authenticates; the authoritative state
  survives and a fresh client reconverges:

  ```bash
  SID=$(RELAY_URL=ws://localhost:18090 node qa-reconnect.mjs start 400 15)
  podman restart deploy_relay_1
  # wait for /health connected_devices=1, then:
  RELAY_URL=ws://localhost:18090 node qa-reconnect.mjs resume "$SID" 400
  ```

## Terminal over E2EE (ghostty-web)

A real interactive PTY (bash) on the bridge, streamed to a ghostty-web terminal in
the browser over the encrypted relay. The bridge keeps an authoritative `vt100`
screen model, so (re)attach sends a screen **snapshot** then live-tails — the
snapshot-resync model, not byte replay. Disconnects are caught by an
application-level liveness ping (a relay/bridge outage does not close the
client↔gateway socket).

```bash
cd web
# Protocol-level verification of all four criteria (real-time, input, disconnect,
# reconnect) over the real relay:
RELAY_URL=ws://localhost:18090 node term-verify.mjs

# Real ghostty terminal in headless Chromium, bouncing the relay for a genuine
# disconnect; writes screenshots to /tmp/term-*.png  (needs: npm install --include=dev)
node term-browser.mjs
```

For a human: serve `web/` (`node web/serve.mjs`) and open `terminal.html` while the
stack is up.
