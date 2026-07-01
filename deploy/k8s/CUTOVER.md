# Cutover: getbuild.ing v2 (namespace `8ly`)

This replaces the old v1 deployment (`build-web` + `build-relay` in namespace
`zechcodes`) with the v2 stack in namespace `8ly`:

- **build-app** — the Skrift app (auth, device registry, gateway tokens, SPA) at
  `getbuild.ing`
- **build-relay** — the Rust ciphertext-only relay at `relay.getbuild.ing`
- **build-postgres** — Postgres 16 on a `do-block-storage-retain` volume

DNS for `getbuild.ing` and `relay.getbuild.ing` **already points at this
cluster** (via the old `zechcodes` ingresses), so there is no DNS step — routing
flips when the old host-based ingresses are removed.

Cluster context: `do-nyc1-production-hosting`.

## 0. One-time prerequisites

- The GHCR packages `ghcr.io/8ly-dev/build-app` and `ghcr.io/8ly-dev/build-relay`
  must be **public** (or add an `imagePullSecrets` to both Deployments and create
  a `ghcr` pull secret in namespace `8ly`).
- Images are published by `.github/workflows/deploy-images.yml` on every push to
  `main`. For a first manual push:

  ```bash
  podman build --target relay -t ghcr.io/8ly-dev/build-relay:latest -f bridge/Containerfile bridge/
  podman build -t ghcr.io/8ly-dev/build-app:latest -f skriftapp/Containerfile .
  podman login ghcr.io
  podman push ghcr.io/8ly-dev/build-relay:latest
  podman push ghcr.io/8ly-dev/build-app:latest
  ```

## 1. Bootstrap secrets (idempotent, never regenerates existing ones)

```bash
deploy/k8s/bootstrap-secrets.sh do-nyc1-production-hosting
```

Creates in namespace `8ly`: `build-postgres` (POSTGRES_PASSWORD), `build-app`
(SECRET_KEY, INTERNAL_API_SECRET, DATABASE_URL), `build-relay`
(RELAY_INTERNAL_SECRET, same value as INTERNAL_API_SECRET). Nothing is ever
committed to git.

## 2. Apply the stack

```bash
kubectl --context do-nyc1-production-hosting apply -k deploy/k8s
kubectl --context do-nyc1-production-hosting -n 8ly get pods -w
```

Expected: `build-postgres-0` ready first, then `build-app` (runs Skrift + buildapp
migrations in its entrypoint, then serves), then `build-relay` ready once its
`/health` probe answers.

## 3. Verify before flipping routing (port-forward, bypasses ingress)

```bash
# App answers (302 to setup/login is success):
kubectl --context do-nyc1-production-hosting -n 8ly port-forward svc/build-app 18080:8080 &
curl -si http://127.0.0.1:18080/ | head -1        # HTTP/1.1 302 (or 200)

# Relay health:
kubectl --context do-nyc1-production-hosting -n 8ly port-forward svc/build-relay 18799:8799 &
curl -si http://127.0.0.1:18799/health | head -1  # HTTP/1.1 200 OK
kill %1 %2
```

## 4. Tear down the old v1 deployment (USER RUNS THIS — not automated)

While both sets of ingresses exist, traefik has **two host-based routes for the
same hosts**, so `getbuild.ing`/`relay.getbuild.ing` traffic and the new
cert-manager HTTP-01 solvers will not settle until the old ingresses are gone.
Delete only the workloads and routing; **old TLS secrets and any volumes stay in
place** (nothing named `pvc`, `pv`, or a database is ever deleted):

```bash
kubectl --context do-nyc1-production-hosting -n zechcodes delete ingress build-web-ingress build-relay-ingress
kubectl --context do-nyc1-production-hosting -n zechcodes delete service build-web build-relay
kubectl --context do-nyc1-production-hosting -n zechcodes delete deployment build-web build-relay
```

## 5. Post-cutover checks

```bash
# cert-manager issues fresh certs for the new ingresses (1-3 min):
kubectl --context do-nyc1-production-hosting -n 8ly get certificate
curl -sI https://getbuild.ing/ | head -1
curl -sI https://relay.getbuild.ing/health | head -1
```

Then:

1. Open `https://getbuild.ing/` and complete the **Skrift setup wizard**
   (first-run only: creates the admin account; sign in with a passkey).
2. **Existing bridges must re-pair**: the v2 api has a fresh device registry, so
   every bridge re-runs the pairing flow against `https://getbuild.ing` and gets
   re-approved before it can connect to `wss://relay.getbuild.ing/ws/device`.

## Rollback

The old `zechcodes` deployment is only scaled away by step 4. Until you run
step 4, rollback is simply `kubectl -n 8ly delete -k deploy/k8s` — the old
ingresses still own the hosts. After step 4, re-create the old ingresses/services
from your last-known manifests (or `kubectl -n zechcodes rollout undo` if only
the deployments were touched). The retained Postgres volume survives any
rollback: the `do-block-storage-retain` PV is never deleted automatically.
