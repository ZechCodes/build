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

## Known-stale harnesses

`web/qa-reconnect.mjs`, `web/term-verify.mjs`, `web/term-browser.mjs` still
speak the pre-auth gateway protocol (no `authenticate` first frame) and need
updating before they run against this stack. `web/e2e.mjs`, `web/qa.mjs`,
`web/skrift-flow.mjs`, and `web/pair.mjs` are current.
