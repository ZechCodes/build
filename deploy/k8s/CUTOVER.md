# Cutover: getbuild.ing v2 (namespace `8ly`)

This replaces the old v1 deployment (`build-web` + `build-relay` in its own
namespace, `<v1-namespace>` below) with the v2 stack in namespace `8ly`:

- **build-app** — the Skrift app (auth, device registry, gateway tokens, SPA) at
  `getbuild.ing`
- **build-relay** — the Rust ciphertext-only relay at `relay.getbuild.ing`
- **build-postgres** — Postgres 16 on a `do-block-storage-retain` volume

DNS for `getbuild.ing` and `relay.getbuild.ing` **already points at this
cluster** (via the old v1 ingresses), so there is no DNS step — routing
flips when the old host-based ingresses are removed.

Cluster context: `do-nyc1-production-hosting`.

## 0. One-time prerequisites

- Both Deployments reference the `ghcr-pull` imagePullSecret, so the GHCR
  packages `ghcr.io/8ly-dev/build-app` and `ghcr.io/8ly-dev/build-relay` may stay
  private. Create it once (never commit the token):

  ```bash
  kubectl --context do-nyc1-production-hosting -n 8ly create secret docker-registry ghcr-pull \
    --docker-server=ghcr.io --docker-username=<gh-user> --docker-password=<token-with-read:packages>
  ```
- Images are published by the deploy stages of `.github/workflows/ci.yml` on
  every push to `main`, after that workflow's checks pass. For a first manual
  push:

  ```bash
  podman build --target relay -t ghcr.io/8ly-dev/build-relay:latest -f bridge/Containerfile bridge/
  podman build -t ghcr.io/8ly-dev/build-app:latest -f skriftapp/Containerfile .
  podman login ghcr.io
  podman push ghcr.io/8ly-dev/build-relay:latest
  podman push ghcr.io/8ly-dev/build-app:latest
  ```

## 1. Bootstrap secrets (idempotent, never regenerates existing ones)

```bash
export SMTP_USERNAME='...' SMTP_PASSWORD='...' \
       SMTP_FROM_ADDRESS='Build <hello@getbuild.ing>' \
       WAITLIST_NOTIFY_ADDRESS='...'
deploy/k8s/bootstrap-secrets.sh do-nyc1-production-hosting
```

Creates in namespace `8ly`: `build-postgres` (POSTGRES_PASSWORD), `build-app`
(SECRET_KEY, INTERNAL_API_SECRET, DATABASE_URL, the VAPID web-push keys, and the
four outbound-email keys), `build-relay` (RELAY_INTERNAL_SECRET, same value as
INTERNAL_API_SECRET). Nothing is ever committed to git.

The four email values are credentials the script cannot invent, so it reads them
from the environment and aborts if any is unset. `SMTP_USERNAME`,
`SMTP_PASSWORD` and `SMTP_FROM_ADDRESS` are required `secretKeyRef`s on the
Deployment: run this step **before** the first deploy that carries the waitlist
email code, or the deploy fails: the migration Job cannot load the config, and a
new pod would fail with `CreateContainerConfigError` (the rolling update keeps
the old pod serving, but nothing new ships).

`CF_TURN_KEY_ID` and `CF_TURN_KEY_API_TOKEN` — the Cloudflare TURN key the
ICE-servers route mints per-user credentials from — cannot be invented either,
but they are optional on the Deployment: export them to have the script patch
them in, and without them the route answers a STUN-only list — which is enough
for peers that can hole-punch and nothing at all for peers that cannot. There is
no relay underneath the peer connection: a browser that cannot reach its bridge
shows that machine as blocked. Their egress has a monthly check in
[`../OPS.md`](../OPS.md). Verify with:

```bash
kubectl --context do-nyc1-production-hosting -n 8ly get secret build-app \
  -o go-template='{{range $key, $unused := .data}}{{$key}} {{end}}'
```

## 2. Apply the stack

```bash
kubectl --context do-nyc1-production-hosting apply -k deploy/k8s
kubectl --context do-nyc1-production-hosting -n 8ly rollout status statefulset/build-postgres --timeout=300s
kubectl --context do-nyc1-production-hosting -n 8ly apply -f deploy/k8s/migrate.yaml
kubectl --context do-nyc1-production-hosting -n 8ly wait --for=condition=complete job/build-app-migrate --timeout=600s
kubectl --context do-nyc1-production-hosting -n 8ly get pods -w
```

Expected: `build-postgres-0` ready first, then the `build-app-migrate` Job
completes (Skrift + buildapp migrations; the app pods never migrate on start,
see `migrate.yaml`), then `build-app` ready once `/readyz` has reached the database,
then `build-relay` ready once its `/health` probe answers. `build-app` may start
before the Job finishes; its pages fail until the schema exists, which is fine
before routing flips. Every later deploy runs the sequence in
[`../OPS.md`](../OPS.md) ("Deploying the app").

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

## 4. Tear down the old v1 deployment (run by hand — not automated)

While both sets of ingresses exist, traefik has **two host-based routes for the
same hosts**, so `getbuild.ing`/`relay.getbuild.ing` traffic and the new
cert-manager HTTP-01 solvers will not settle until the old ingresses are gone.
Delete only the workloads and routing; **old TLS secrets and any volumes stay in
place** (nothing named `pvc`, `pv`, or a database is ever deleted):

```bash
kubectl --context do-nyc1-production-hosting -n <v1-namespace> delete ingress build-web-ingress build-relay-ingress
kubectl --context do-nyc1-production-hosting -n <v1-namespace> delete service build-web build-relay
kubectl --context do-nyc1-production-hosting -n <v1-namespace> delete deployment build-web build-relay
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

The old v1 deployment is only scaled away by step 4. Until you run
step 4, rollback means removing **only the v2 workloads and routing** — the old
ingresses still own the hosts:

```bash
kubectl --context do-nyc1-production-hosting -n 8ly delete ingress build-app build-relay
kubectl --context do-nyc1-production-hosting -n 8ly delete deployment build-app build-relay
# Optional: also stop Postgres WITHOUT touching its volume or data:
kubectl --context do-nyc1-production-hosting -n 8ly scale statefulset build-postgres --replicas=0
```

**Never `kubectl -n 8ly delete -k deploy/k8s`**: `namespace.yaml` is in the
kustomization, so `delete -k` deletes the whole `8ly` namespace — cascading to
the bootstrap Secrets (the Postgres password is only honored at initdb and is
otherwise unrecoverable against the retained PGDATA; the VAPID keys orphan every
push subscription) and the `data-build-postgres-0` PVC. Same rule as step 4:
nothing named `pvc`, `pv`, a Secret, or a database is ever deleted.

After step 4, re-create the old ingresses/services from your last-known
manifests (or `kubectl -n <v1-namespace> rollout undo` if only the deployments were
touched). The retained Postgres volume survives any rollback: the
`do-block-storage-retain` PV is never deleted automatically.

## HTTPS note

Both v2 ingresses pin `traefik.ingress.kubernetes.io/router.entrypoints:
websecure`, so the app and relay are never served over plain HTTP regardless of
cluster-wide traefik config. If you want `http://getbuild.ing` to *redirect*
(rather than 404), verify the cluster's traefik has a global `web` →
`websecure` redirection; do not loosen the ingress annotation.
