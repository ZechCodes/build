# Hosted dev stack

This resource set is isolated in `build-dev`: `https://dev.getbuild.ing/app/`,
`wss://relay-dev.getbuild.ing` and namespace-local secrets. Dev shares the
existing PostgreSQL server at `build-postgres.8ly.svc.cluster.local:5432`, using
database `build`, dedicated login role `build_dev` and schema `build_dev`.
There is no dev Postgres Service, StatefulSet, PVC or connection pool. Dev's
credentials, schema, accounts and public origins are separate from production.

`namespace.yaml` is **operator bootstrap only** and is intentionally absent
from `kustomization.yaml`. An authorized operator provisions the database role
and schema, namespace, namespace-scoped RBAC, the `development` GitHub
environment restricted to `main` (no required reviewers), and these secrets
before the first deployment. Zech handles DNS; do not change it in this procedure.

- `build-app`: fresh `SECRET_KEY`, `INTERNAL_API_SECRET`, and `DATABASE_URL`
  pointing to `build-postgres.8ly.svc.cluster.local:5432/build` with the
  dedicated dev password
  (`postgresql+asyncpg://build_dev:<encoded-password>@build-postgres.8ly.svc.cluster.local:5432/build`).
  Do not use the production `build` role or add URL query options.
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
Deployment, Service, Ingress, Job and pod/log/rollout read operations
needed by this workflow. It has no direct Secret API access, Namespace/RBAC
permissions, pod-exec permission or production namespace permissions. However,
Job/Deployment create plus pods/log is equivalent to reading the dev secrets:
a workload can mount them and print them. This is accepted because only
dev-scoped credentials live in `build-dev` and the `development` environment is
restricted to workflow refs on `main`, with no required reviewers. The
operator-only `ci-rbac.yaml` defines this ServiceAccount, Role and RoleBinding; it is excluded from the CI
render. Create its token separately, store the kubeconfig only in the protected
environment, and verify permissions with that actual credential, including
denial of pod exec and access to `8ly`. CI never bootstraps these permissions or
secrets. Repository package permissions must allow publishing `build-app-dev`
and `build-relay-dev`. Build/check jobs receive no kube credential.

CI's Ingress create/update/patch verbs could claim any Traefik host, including a
production host. The reviewed renderer rejects unexpected hosts, but it is
checked out from `dev`, so dev writers can change it. Treat dev push access as authority over the
dev workloads and credentials; namespace-scoped RBAC does not restrict Ingress
hostnames. This flow does not expand the existing dev credential's permissions.

For the local dev bridge, authorized agents use only
[`buildctl`](../../../tools/buildctl/README.md) (`scripts/buildctl --env dev …`). See
[Working against dev](../../../AGENTS.md#working-against-dev) for deploy, pairing,
verification, status/logs, rollback and pruning. Validate changes on this dev
stack before production; production bridge authorization is unchanged.

## Shared database preflight and provisioning

Before creating the role or schema, inspect privileges, capacity and networking
using the existing operator credential. Never print database credentials. Do not
edit, restart or scale resources in `8ly`, or alter production data, roles,
objects, grants or NetworkPolicies.

- Confirm PUBLIC does not grant access to production tables or sequences, and
  inspect default privileges and custom functions, especially SECURITY DEFINER
  functions. Schema USAGE alone does not grant table access. If inherited access
  exposes production data, stop before creating anything or running migrations;
  do not revoke production grants. A separate `build_dev` database on the same
  server is the approved fallback, but requires a reviewed update to this exact
  database guard. A separate database alone does not remove inherited PUBLIC
  CONNECT or data privileges in `build`; resolve any remaining access before
  proceeding.
- Check `max_connections`, current connections and resource headroom. Create
  only the new `build_dev` login role with `CONNECTION LIMIT 10`, NOSUPERUSER,
  NOCREATEDB, NOCREATEROLE, NOREPLICATION and NOBYPASSRLS, with no memberships in
  existing roles. The app uses a two-connection pool with no overflow.
- Grant the new role CONNECT on `build`; create schema `build_dev` owned by
  `build_dev`; set that role's default `search_path` to `build_dev`. Grant no
  rights on production objects. Use a fresh password through private input,
  never a shell argument, printed SQL, task comment or log. Abort if the role or
  schema already exists and its provenance is unknown.
- Confirm a client using the dev role can reach the service from `build-dev`,
  owns only the dev schema, cannot read/write production tables, and resolves
  `current_schema()` to `build_dev`. If an `8ly` NetworkPolicy blocks access,
  stop and ask; do not edit that policy. This is an internal cluster connection
  to the existing service, without a new pool or changes to server TLS settings.

Only the new role, its own settings and grants, and the new schema are required
on the shared server. Run migrations exclusively through the guarded dev
command below. Its checked connection and Alembic version table are confined
to `build_dev`; do not run the generic `skrift db upgrade` command against this
shared database. Runtime ORM models remain unqualified and resolve through the
restricted role's verified `search_path = build_dev`. Keep `db.schema` unset:
Skrift imports its framework tables before applying that metadata default, which
otherwise leaves Build's foreign keys pointing at a missing `build_dev.users`
metadata entry. The operator engine retains its explicit schema translation;
database privileges and the guarded preflight enforce the isolation boundary.

## Automatic deployment and branch ownership

A push (including a force-push) to `refs/heads/dev` starts the secret-free
`dev-deploy-request.yml` workflow, named `Dev deploy request`. It has an empty
permission set, no checkout, no environment, no inputs and no artifacts. Its
successful completion starts `dev-deploy.yml` through `workflow_run` on the
repository's default branch, which must remain `main`.
[GitHub's workflow_run contract](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_run)
uses the default branch's workflow definition and allows it to access its own
secrets. The main-side entry gate additionally requires `refs/heads/main`, a
successful `push` event on `dev`, and the same head repository. A manual dispatch
on `main` remains available for retries; it is not part of the normal dev flow.
No development environment approval is required; its `main`-only branch policy
stays in place. Production workflows, environments and manifests are unchanged.

Agents and the orchestrator with repository write access may move `dev`, just as
they push task branches. Force-push is intentionally allowed for testing a task
branch; prefer `--force-with-lease` to avoid overwriting another writer's update.
Keep the signal workflow in commits pushed to `dev`: deleting, renaming or
breaking it can stop notifications, but cannot replace the workflow definition
executed on `main`. Use a normal authenticated git credential (a user token or
GitHub App token), not a workflow's `GITHUB_TOKEN`, whose pushes do not trigger
other workflows ([GitHub token behavior](https://docs.github.com/en/actions/concepts/security/github_token)). Neither branch protection nor credentials are changed by this
implementation. Rollout requires merging these workflow changes to `main` and
including the signal file in the next dev tip; no dispatch is needed afterward.

The signal carries no deployment instructions. Main ignores its SHA, outputs,
artifacts, display names and inputs: it independently resolves the current dev
branch. Delayed or reordered signals therefore deploy the current tip at resolution rather
than a stale signal SHA. Changing the dev copy of `dev-deploy.yml` cannot change
the main-side workflow. Dev source code, dependency hooks, Containerfiles,
manifests and the renderer still execute in that reviewed pipeline; dev pushers
are trusted with dev-only credentials and images. This is not a sandbox for
hostile commits. Do not add production secrets to this workflow or environment.

The `dev-deploy.yml` workflow resolves the `dev` branch once, records its
full SHA, and checks out that SHA for all checks, image builds and deployment
rendering. It publishes only `build-app-dev:<SHA>` and `build-relay-dev:<SHA>`;
rollouts use the returned immutable digests, never a mutable tag. Re-dispatching
can rebuild the same SHA; the new run still deploys only its own build's digest.
The Containerfile defaults remain production (`APP_CONFIG=app.yaml`,
`VITE_BUILD_CHANNEL=prod`); the dev workflow explicitly supplies
`APP_CONFIG=app.channel-dev.yaml`, `VITE_BUILD_CHANNEL=dev`, the dev relay URL
and the resolved source SHA.

Both build jobs require the checks to succeed; failed, skipped or cancelled
checks prevent publishing and deployment. New runs cancel older checks and image
builds through their respective concurrency groups. There is deliberately no
workflow-level cancellation: it would override rollout protection. Each rollout
waits only for its own successful image and has its own non-cancelling concurrency
group (`deploy-app-dev` or `deploy-relay-dev`). Once migration/rollout starts, it
finishes before the next ready job in that group runs. GitHub concurrency keeps
at most one pending job per group and may replace a pending job; intermediate
tips can be superseded rather than deployed. App and relay remain independent.
A partial failure or manual cancellation can still need operator recovery, but a
new push does not interrupt an active migration or rollout.

At the start of each rollout job, **inside its non-cancelling concurrency group**,
a read-only GitHub API request re-reads `refs/heads/dev`. If its tip differs from
the run's resolved SHA, the job reports `superseded by <tip>` in its summary and
skips every remaining step, including checkout, rendering, credential setup,
migration and rollout. An API error or malformed SHA fails closed. A delayed old
job therefore cannot overwrite a newer deployment: it must still match dev when
it acquires the group. A tip that moves after this check lets the active rollout
finish; the newer run follows it. This check does not change the selected source
SHA or consume anything from the signal workflow. GitHub does not guarantee FIFO
scheduling; superseded snapshots are deliberately skipped.
`kubectl kustomize deploy/k8s/dev` is an offline render. `deploy/render_dev.py`
refuses cluster/bootstrap resources, unexpected names/namespaces and source
images, and writes only the phases selected by that rollout:

1. App: recreate the dev migration Job with the app digest and wait for
   completion. It validates the dev database identity and upgrades only the dev
   schema and checks the operator bootstrap marker/invite before permitting the
   rollout; a fresh unbootstrapped database fails closed. Then apply the
   dev app/Service/Ingress and wait for readiness. Migration failure stops the
   app rollout. Migrations must remain compatible with the previous app image.
2. Relay: apply dev relay/Service/Ingress with its own built digest and wait for
   readiness. `Recreate` preserves the single in-memory broker topology.

First deployment may require retrying the relay after the app becomes available.
Failures leave production untouched; fix forward by pushing a corrected dev
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

This is an operator procedure, **never a CI step**. Complete it before pushing
the first deployable dev tip; automatic dev rollouts have no approval pause.
The `development` environment restricts workflow refs to `main` and has no
required reviewers. Prepare the namespace, RBAC,
secrets, database role/schema and DNS/TLS from the provisioning checklist first.
Do not apply the app phase or create its public Ingress during bootstrap.
The relay is independent and can roll out before app bootstrap completes.
There must be no existing app Deployment/Ingress for a fresh database. If an old
dev stack points at a replacement empty database, withdraw its public exposure
and stop the old app before beginning; the new migration gate cannot protect an
already-running app.

Use one operator, one fresh dev schema and the reviewed source commit that will
be deployed. Obtain an immutable `ghcr.io/zechcodes/build-app-dev@sha256:…` image
built with `APP_CONFIG=app.channel-dev.yaml` from that commit. A first
workflow's successful `build-app` output can supply this digest: on a fresh database the app migration/bootstrap check fails closed before any public
app exposure. There is no approval pause; the relay can roll out independently.
Do not bypass the app's bootstrap check merely to obtain the image. The operator
credential needs dev-only pod create/exec/delete for the bootstrap pod; CI's credential must not gain exec access.
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
invite_root="$HOME/.build/project-scratch/dev-invite"
mkdir -p "$invite_root"
chmod 700 "$invite_root"
invite_dir="$(mktemp -d "$invite_root/first-boot.XXXXXXXX")"

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

# The role/schema already exist on the shared server. Never apply app.yaml here.
kubectl --context="$DEV_CONTEXT" -n build-dev create -f "$bootstrap_dir/operator-pod.yaml"
kubectl --context="$DEV_CONTEXT" -n build-dev wait --for=condition=Ready pod/build-dev-bootstrap --timeout=300s

# Required order: migrate -> fresh invite in private file -> setup completion.
kubectl --context="$DEV_CONTEXT" -n build-dev exec build-dev-bootstrap -- \
  python -m buildapp.dev_bootstrap migrate
# Both the temporary pod copy and operator copy stay under private scratch.
kubectl --context="$DEV_CONTEXT" -n build-dev exec build-dev-bootstrap -- \
  mkdir -p -m 700 /home/appuser/.build/project-scratch/dev-invite
kubectl --context="$DEV_CONTEXT" -n build-dev exec build-dev-bootstrap -- \
  python -m buildapp.dev_bootstrap bootstrap \
  --invite-file /home/appuser/.build/project-scratch/dev-invite/first-invite.txt
kubectl --context="$DEV_CONTEXT" -n build-dev exec build-dev-bootstrap -- \
  python -m buildapp.dev_bootstrap check

# Redirect directly to a 0600 file inside the operator's private directory.
# This is the only raw-link transfer; never use kubectl logs to retrieve it.
kubectl --context="$DEV_CONTEXT" -n build-dev exec build-dev-bootstrap -- \
  cat /home/appuser/.build/project-scratch/dev-invite/first-invite.txt > "$invite_dir/invite.txt"
test -s "$invite_dir/invite.txt"
test "$(stat -c %a "$invite_dir/invite.txt")" = 600
kubectl --context="$DEV_CONTEXT" -n build-dev delete pod build-dev-bootstrap --wait=true
```

The bootstrap command refuses a database with users, invites or completed setup;
it exclusively creates a mode-0600 file and calls `issue_open_invite`. The DB
stores only the token hash. After the file is flushed to storage, it writes
`settings.setup_completed_at` as a UTC ISO timestamp. The rollout check reads
that marker plus evidence of an invite; it never seeds or prints credentials.
`SKRIFT_ENV` remains `production`; the command rejects the production origin and
any Postgres target except `build-postgres.8ly.svc.cluster.local/build`
with user `build_dev`, no configured metadata schema, port 5432 and no query
options (which could override the connection target). It checks the effective
role, schema ownership and search path before migration or bootstrap. SQLite is
permitted only for disposable offline proof, without a configured schema.

Check every command's exit status. On failure keep the app unexposed and do not
push another deployable tip. Do not blindly rerun bootstrap or delete the
pod/private file: invite creation commits before file delivery and setup completion. If DB
or file persistence fails, setup remains closed and a retry refuses the existing
invite. An operator must inspect the private file/state and either recover it or
recreate the **dev schema only** through a separately approved recovery, after
checking no dev accounts exist, then restart with a new output file. Do not delete prod data or automatically
reset a populated dev database. If only local transfer failed, retry the transfer
from the still-running pod; the pod expires after one hour. If the link is lost or
expires, use the existing invite administration service to revoke/reissue under
operator control; never reset setup on a running stack. A failed first workflow
migration check can be rerun only after this procedure is complete.

After successful private delivery and pod cleanup, report only the local invite
file's path. Then push the intended dev tip (or manually retry the main-side
deploy workflow after bootstrap if the first run failed its bootstrap check).
CI repeats migrations and the bootstrap check before applying the app
Service/Deployment/Ingress. Verify `/healthz` and `/readyz` return 200,
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
