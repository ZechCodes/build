# Running Build v2 over the real relay

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
```

`provision.py` mints a UUID device id + Ed25519 identity key + X25519 transport
key. The padded Ed25519 public key is seeded into `build.devices` (approved) so
the relay authenticates the bridge; the private keys go to the bridge; the
transport public key + device id go to the gateway. Secrets live in
`deploy/secrets/` (git-ignored).

## Up / verify / down

```bash
podman compose -f deploy/compose.real.yml up -d --build
curl -s localhost:18081/health           # {"connected_devices": 1}
node web/qa-reconnect.mjs                 # disconnect/reconnect verification
podman compose -f deploy/compose.real.yml down
```
