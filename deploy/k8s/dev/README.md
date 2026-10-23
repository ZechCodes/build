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
  separate `VAPID_PRIVATE_KEY`, `VAPID_PUBLIC_KEY`, `VAPID_SUBJECT`. Use a dedicated dev
  FastMail account/app password (smtp.fastmail.com:587 with STARTTLS). Optional `CF_TURN_KEY_ID`/`CF_TURN_KEY_API_TOKEN` enable TURN;
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
   migration Job with the app digest and wait for completion. It upgrades the
   schema and checks the operator bootstrap marker/invite before permitting the
   rollout; a fresh unbootstrapped database fails closed. Then apply the
   dev app/Service/Ingress and wait for readiness. Migration failure stops the
   app rollout. Migrations must remain compatible with the previous app image.
2. Relay: apply dev relay/Service/Ingress with its own built digest and wait for
   readiness. `Recreate` preserves the single in-memory broker topology.

First deployment may require retrying the relay after the app becomes available.
Failures leave production untouched; fix forward by dispatching an approved dev
commit. An operator-directed rollback must select a previously reviewed dev
commit/digest and account for schema compatibility; no automatic database
rollback is attempted. Complete the first-boot procedure below before the app
rollout; create the account/passkey only after bootstrap and rollout succeed.
Never copy production identities, databases or browser credentials into dev.

Local proof uses two disposable API stores with the real auth/ownership guards,
production-strength config/header tests, and a Chromium test with two loopback
origins exercising equal localStorage/IndexedDB/cache/BroadcastChannel/worker
names. It does not provision or validate live DNS, RBAC, secrets or TLS; #785
validates all three implementation slices before any deployment is authorized.

## First boot: operator bootstrap before public app exposure

This is an operator procedure, **never a CI step**. Keep both first-run rollout
approvals pending until it finishes. The `development` environment must require
reviewers and restrict workflow refs to `main`. Prepare the namespace, RBAC,
secrets, DNS/TLS and storage from the provisioning checklist first. Do not apply
the app or relay phases, or create their public Ingresses, during bootstrap.
There must be no existing app Deployment/Ingress for a fresh database. If an old
dev stack points at a replacement empty database, withdraw its public exposure
and stop the old app before beginning; the new migration gate cannot protect an
already-running app.

Use one operator, one fresh dev database and the reviewed source commit that will
be deployed. Obtain an immutable `ghcr.io/zechcodes/build-app-dev@sha256:…` image
built with `APP_CONFIG=app.channel-dev.yaml` from that commit. The first manual
workflow's successful `build-app` output can supply this digest **while both
protected deploy jobs remain unapproved**. Do not approve a deployment merely
to obtain the image. The operator credential needs dev-only pod create/exec/delete
in addition to infrastructure apply; CI's credential must not gain exec access.
The bootstrap pod receives only the namespace-local `build-app` secret, inherits
the migration pod's non-root restrictions and pull secret, and serves no HTTP.

On a private operator terminal, from the reviewed checkout, set `DEV_CONTEXT` to
the verified dev kube context and `APP_IMAGE` to that digest. These commands are
instructions for the separately authorized operator; validation of this change
runs offline only. Do not enable shell tracing/session recording or paste the
invite into task comments, logs, screenshots, CI artifacts or shell arguments.

```bash
set -eu
set +x
umask 077
: "${DEV_CONTEXT:?set the verified dev context}"
: "${APP_IMAGE:?set the reviewed build-app-dev digest}"
bootstrap_dir="$(mktemp -d)"

# Offline: pin the reviewed digest and derive a private operator pod from the
# migration template, so secret refs, security and image cannot drift.
nice -n 10 kubectl kustomize deploy/k8s/dev > "$bootstrap_dir/resources.yaml"
nice -n 10 uv run --project skriftapp --frozen python deploy/render_dev.py \
  --input "$bootstrap_dir/resources.yaml" --output "$bootstrap_dir/phases" \
  --target app --app-image "$APP_IMAGE"
nice -n 10 uv run --project skriftapp --frozen python - "$bootstrap_dir" <<'PYTHON'
from pathlib import Path
import sys
import yaml
root = Path(sys.argv[1])
job = yaml.safe_load((root / 'phases/migrate.yaml').read_text())
spec = job['spec']['template']['spec']
spec['containers'][0]['command'] = ['sleep', '3600']
spec['automountServiceAccountToken'] = False
spec['activeDeadlineSeconds'] = 3600
pod = {'apiVersion': 'v1', 'kind': 'Pod',
       'metadata': {'name': 'build-dev-bootstrap', 'namespace': 'build-dev'},
       'spec': spec}
(root / 'operator-pod.yaml').write_text(yaml.safe_dump(pod))
PYTHON

# Apply infrastructure only. Never apply phases/app.yaml at this point.
kubectl --context="$DEV_CONTEXT" -n build-dev apply -f "$bootstrap_dir/phases/infrastructure.yaml"
kubectl --context="$DEV_CONTEXT" -n build-dev rollout status statefulset/build-postgres --timeout=300s
kubectl --context="$DEV_CONTEXT" -n build-dev create -f "$bootstrap_dir/operator-pod.yaml"
kubectl --context="$DEV_CONTEXT" -n build-dev wait --for=condition=Ready pod/build-dev-bootstrap --timeout=300s

# Required order: migrate -> fresh invite in private file -> setup completion.
kubectl --context="$DEV_CONTEXT" -n build-dev exec build-dev-bootstrap -- skrift db upgrade heads
kubectl --context="$DEV_CONTEXT" -n build-dev exec build-dev-bootstrap -- \
  python -m buildapp.dev_bootstrap bootstrap --invite-file /tmp/dev-first-invite.txt
kubectl --context="$DEV_CONTEXT" -n build-dev exec build-dev-bootstrap -- \
  python -m buildapp.dev_bootstrap check

# Redirect directly to a 0600 file inside the operator's private directory.
# This is the only raw-link transfer; never use kubectl logs to retrieve it.
kubectl --context="$DEV_CONTEXT" -n build-dev exec build-dev-bootstrap -- \
  cat /tmp/dev-first-invite.txt > "$bootstrap_dir/invite.txt"
test -s "$bootstrap_dir/invite.txt"
kubectl --context="$DEV_CONTEXT" -n build-dev delete pod build-dev-bootstrap --wait=true
```

The bootstrap command refuses a database with users, invites or completed setup;
it exclusively creates a mode-0600 file and calls `issue_open_invite`. The DB
stores only the token hash. After the file is flushed to storage, it writes
`settings.setup_completed_at` as a UTC ISO timestamp. The rollout check reads
that marker plus evidence of an invite; it never seeds or prints credentials.
`SKRIFT_ENV` remains `production`; the command rejects the production origin and
any Postgres target except `build-postgres.build-dev.svc.cluster.local/build`
with user `build`, port 5432 and no query options (which could override the
connection target). SQLite is permitted for disposable offline tests.

Check every command's exit status. On failure keep the app unexposed and leave
the approvals pending. Do not blindly rerun bootstrap or delete the pod/private
file: invite creation commits before file delivery and setup completion. If DB
or file persistence fails, setup remains closed and a retry refuses the existing
invite. An operator must inspect the private file/state and either recover it or
recreate the **disposable dev database only**, after checking no dev accounts
exist, then restart with a new output file. Do not delete prod data or automatically
reset a populated dev database. If only local transfer failed, retry the transfer
from the still-running pod; the pod expires after one hour. If the link is lost or
expires, use the existing invite administration service to revoke/reissue under
operator control; never reset setup on a running stack. A failed first workflow
migration check can be rerun only after this procedure is complete.

After successful private delivery and pod cleanup, approve the reviewed app/relay
rollouts. CI repeats migrations and the read-only bootstrap check before applying
the app Service/Deployment/Ingress. Verify `/healthz` and `/readyz` return 200,
`/auth/login` renders and `/setup` and `/setup/welcome` return 404. Deliver the link
from the private file directly to the intended tester using an approved private
channel. Open it at `https://dev.getbuild.ing/invite/<token>`, create a separate dev
account, and register a **new passkey for RP `dev.getbuild.ing`**. It is a one-use
open invite, expires after 14 days and binds to its first redeemer; do not share it
with multiple testers. This creates an ordinary invited dev account, not an
administrator. Remove the private file after delivery/redemption under your
credential-handling policy. Do not copy prod passkeys, accounts or sessions.
Bridge onboarding remains separate and subject to the dev installation guidance;
bootstrap does not run an installer or pair a bridge.
