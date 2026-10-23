# Hosted dev stack

This resource set is isolated in `build-dev`: `https://dev.getbuild.ing/app/`,
`wss://relay-dev.getbuild.ing`, a separate Postgres 16 StatefulSet/PVC and
namespace-local secrets. Resource names may match production; their namespace,
volume claims, credentials, accounts and public origins must not.

`namespace.yaml` is **operator bootstrap only** and is intentionally absent
from `kustomization.yaml`. Zech provisions the namespace, namespace-scoped RBAC,
DNS, storage capacity, `development` GitHub environment with required reviewers,
and these secrets before authorizing the first manual deployment:

- `build-postgres`: fresh `POSTGRES_PASSWORD` for database/user `build`.
- `build-app`: fresh `SECRET_KEY`, `INTERNAL_API_SECRET`, and `DATABASE_URL`
  pointing to `build-postgres.build-dev.svc.cluster.local:5432/build` with the
  dev password (`postgresql+asyncpg://build:<encoded-password>@.../build`).
  Include dev `SMTP_USERNAME`, `SMTP_PASSWORD`, `SMTP_FROM_ADDRESS` (required by
  the production-derived config), optional `WAITLIST_NOTIFY_ADDRESS`, and a
  separate `VAPID_PRIVATE_KEY`, `VAPID_PUBLIC_KEY`, `VAPID_SUBJECT`. Use a dev
  mail account/sink. Optional `CF_TURN_KEY_ID`/`CF_TURN_KEY_API_TOKEN` enable TURN;
  without them the API returns STUN-only configuration.
- `build-relay`: `RELAY_INTERNAL_SECRET`, matching only the dev app's
  `INTERNAL_API_SECRET`.
- `ghcr-pull`: namespace-local read credentials for the two dev image repos.
- TLS secrets `dev-getbuild-ing-tls` and `relay-dev-getbuild-ing-tls`, issued for
  their respective hosts by the configured cluster issuer.

The protected `development` environment supplies `DEV_KUBECONFIG` as raw
kubeconfig text. Its principal must be restricted to `build-dev` and only the
Deployment, StatefulSet, Service, Ingress, Job and pod/log/rollout read operations
needed by this workflow. It must have no Namespace, RBAC, Secret-write or
production namespace permissions. CI never bootstraps those permissions or
secrets. Repository package permissions must allow publishing `build-app-dev`
and `build-relay-dev`. Build/check jobs receive no kube credential.

The manual `dev-deploy.yml` workflow resolves the `dev` branch once, records its
full SHA, and checks out that SHA for all checks, image builds and deployment
rendering. It publishes only `build-app-dev:<SHA>` and `build-relay-dev:<SHA>`;
rollouts use the returned immutable digests, never a mutable tag. Re-dispatching
can rebuild the same SHA; the new run still deploys only its own build's digest.
The Containerfile defaults remain production (`APP_CONFIG=app.yaml`,
`VITE_BUILD_CHANNEL=prod`); the dev workflow explicitly supplies
`APP_CONFIG=app.channel-dev.yaml`, `VITE_BUILD_CHANNEL=dev`, the dev relay URL
and the resolved source SHA.

Both build jobs require the checks to succeed. Each protected rollout waits only
for its own successful image and has its own non-cancelling concurrency group.
`kubectl kustomize deploy/k8s/dev` is an offline render. `deploy/render_dev.py`
refuses cluster/bootstrap resources, unexpected names/namespaces and source
images, and writes only the phases selected by that rollout:

1. App: apply dev Postgres/Service and wait for its rollout; recreate the dev
   migration Job with the app digest and wait for completion; then apply the
   dev app/Service/Ingress and wait for readiness. Migration failure stops the
   app rollout. Migrations must remain compatible with the previous app image.
2. Relay: apply dev relay/Service/Ingress with its own built digest and wait for
   readiness. `Recreate` preserves the single in-memory broker topology.

First deployment may require retrying the relay after the app becomes available.
Failures leave production untouched; fix forward by dispatching an approved dev
commit. An operator-directed rollback must select a previously reviewed dev
commit/digest and account for schema compatibility; no automatic database
rollback is attempted. Seed a fresh dev invite/account and passkey afterwards.
Never copy production identities, databases or browser credentials into dev.

Local proof uses two disposable API stores with the real auth/ownership guards,
production-strength config/header tests, and a Chromium test with two loopback
origins exercising equal localStorage/IndexedDB/cache/BroadcastChannel/worker
names. It does not provision or validate live DNS, RBAC, secrets or TLS; #785
validates all three implementation slices before any deployment is authorized.
