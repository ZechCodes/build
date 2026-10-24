# buildctl

One explicit environment for local bridges and hosted app/relay deployments.
From the repository checkout:

```bash
uv run --frozen --project tools/buildctl buildctl --env dev status
scripts/buildctl --env dev bridge status
```

The wrapper runs the same locked uv project. Requires Python 3.11+, uv, Linux
and a systemd user manager. Bridge builds also need Git, Cargo and `nice`; hosted
status needs `gh` authenticated to the repository. Runtime has no third-party
Python dependencies. A `ServiceManager` protocol isolates systemd; launchd is
not implemented yet and unsupported platforms refuse bridge management.

## Environments and authorization

`environments.toml` is the single registry: channel, service name/manager,
state/log/binary paths, API/web/relay origins, deployment branch and workflow,
repository and its default branch, minimum supported bridge commit, and mutation policy. No environment
is implicit, even for reads. The cutover command below names both environments
with `--from prod --to dev` instead of `--env`. Both dev and prod are seeded; prod permits only
status and bridge logs. Prod bridge rolls and prod deployments still need
Zech's authorization and keep their existing path outside this CLI. No CLI flag
can override that restriction. Registry loading rejects a mutable environment
whose deployment branch is `main` or the registered repository `default_branch`
(default `main`). Keep that metadata in sync when the repository default changes;
the guard also runs before mutation so `app deploy` cannot target either branch.

Adding an environment requires a registry entry **and matching bridge channel
support**. Today the bridge accepts only `dev` and `prod`. `registry.py` validates
those channels' actual state roots, log paths and service names; extend those
checks alongside bridge support for a future channel. `CHANNELS` is deliberately
fixed to the channels implemented by the bridge: adding a TOML entry alone does
not add channel support; update both the bridge and `registry.py`'s table. Registry origins must be
public HTTPS/WSS URLs without credentials. Environment roots must be disjoint;
managed paths must remain inside their own root. Symlink aliases and paths into
another environment are refused. The systemd unit remains in systemd's normal
user configuration directory.

Dev state and logs live in `~/.local/state/build-dev/`, with builds under
`builds/<full-commit>/build-bridge`, verification records in `buildctl.json`,
a first-run candidate in `buildctl-candidate`, and a management lock in
`buildctl.lock`. No identity contents are read or printed. Known secret fields
and inherited credential values are redacted from public output. Failed Cargo
builds expose a sanitized diagnostic tail; other subprocess failure diagnostics
are suppressed. Pairing emits only the expected public
approve link while the bridge waits for approval. All inherited `BRIDGE_*`
variables are removed from both Cargo build and bridge invocations; channel and origins are
set from the registry. The parent shell's environment stays unchanged.

## Bridge commands

```bash
scripts/buildctl --env dev bridge deploy             # origin/dev; fetch first
scripts/buildctl --env dev bridge deploy <ref>
scripts/buildctl --env dev bridge pair
scripts/buildctl --env dev bridge status
scripts/buildctl --env dev bridge logs
scripts/buildctl --env dev bridge logs -f
scripts/buildctl --env dev bridge rollback
scripts/buildctl --env dev bridge prune --keep 3
```

Deploy resolves the ref to a full commit, refuses commits predating the required
dev isolation fixes, builds in a temporary detached worktree with
`nice -n 10 cargo build --release --locked --bin build-bridge`, stages the binary
atomically, and removes the worktree. Successful builds stay quiet; failed builds
show the last 40 sanitized stderr lines so compiler errors can be diagnosed.
Individual lines over 4 KiB are omitted before redaction to keep huge generated
source lines from stalling diagnostic handling.
Staged binaries are immutable and reused for the same commit. Installer
stdout/stderr are suppressed. After installation,
systemd is reloaded, the dev unit is stopped, log positions are sampled, and the
unit starts again. Up to 60 seconds are allowed for an active unit, the running
executable's SHA-256 matching the staged binary, and a new
`relay: authenticated` log entry. Rotation/truncation resets the log position.
Only then does one atomic record advance current/previous. Mutating bridge
operations are serialized with a nonblocking lock.

On an unpaired machine, the first deploy stages a candidate and fails the install
gate. Run pair, approve the link on dev, then deploy the same ref again. The
candidate remains available after install/verification failure. `paired` in
status means identity-file existence, not confirmed server approval.

Status reports the real `/proc/<pid>/exe` path and digest even when it is a
legacy binary outside the managed tree. Such a process is marked unmanaged,
with no invented commit. Taking over the legacy service records the first
verified current commit with previous unset; neither deploy nor prune deletes
its externally staged binary.

Verification failure leaves recorded current/previous unchanged, while the unit
or running executable may have changed. Use status/logs to inspect it, then
redeploy the recorded current ref to recover. `rollback` reinstalls the recorded
previous build and performs the same verification; it requires a previous
verified deployment. After a successful rollback, the previous/current records
swap, allowing rollback to be reversed. Rollback replaces the pairing candidate
with its target before installation, so even a failed attempt cannot leave pair
pointing at a stale deployment candidate. There is no automatic rollback. Prune
keeps current, previous, any running managed build, and N newest build directories
(default 3); it removes only directories inside the environment's binary root.

## Prod to dev state cutover

`buildctl migrate --from prod --to dev` is a reversible state migration. It
never starts or stops a unit. **Run the live cutover only in a window Zech picks.**
These commands are an operator runbook, not authorization for an agent to access
live state or services. Do not test against live roots; pytest uses temporary
homes and the separately authorized snapshot is for offline rehearsal only.

The source is `~/.build`; the target is `~/.local/state/build-dev`. Dev keeps its
own identity and pairing. Migration copies config, projects, provider recovery,
review receivers, task state and attachments, resume rosters and local harness
sessions. It uses SQLite's online backup API, rewrites prod state paths, verifies
table counts/record IDs, worktree references and attachment hashes, and retains
a backup for rollback. Logs, builds, deployments, rollouts, backups and project
scratch are excluded, except exact database-referenced project scratch directories
whose names match `<Project>-<12hex>`; those referenced directories are copied.
Missing referenced worktrees are a hard verification failure. Rehearsal reads
the supplied snapshot and checks existence of shared worktree directories;
it never edits shared repository configuration. Review receiver Git URLs in shared repositories also need
the reversible migration update. Source repositories and worktrees under
`~/Projects` and `~/Projects/.build-worktrees` stay in place.

The migration writes `migration-roots.json` in dev state. Subsequent buildctl
bridge installation reads it to set `BRIDGE_PROJECTS_DIR` and `BRIDGE_WORKTREES`.
Shared roots must be absolute paths without symlink aliases and outside either
state root. An existing installed unit does not acquire these settings until
`bridge deploy` installs it again.

At the selected window, from a checkout containing this command:

1. Inspect the dry-run counts and refusals. Dry-run changes no persistent state
   (SQLite may update transient WAL shared-memory reader marks):

   ```bash
   scripts/buildctl migrate --from prod --to dev --dry-run
   ```

2. The operator stops both bridges and confirms both report `inactive`.
   `systemctl is-active` returns exit 3 for inactive units; any other state needs
   investigation before continuing. Neither bridge may manage the shared roots
   during the final copy:

   ```bash
   systemctl --user stop build-bridge.service build-bridge-dev.service
   systemctl --user is-active build-bridge.service
   systemctl --user is-active build-bridge-dev.service
   ```

3. Apply and save the printed backup path and verification report. Keep the
   sibling `<backup-path>.json` rollback manifest with the backup directory. A non-fresh
   target is refused unless its replacement is intentional; add `--force` only
   after reviewing what will be replaced. `--prod-stopped` is an assertion in
   addition to the checked service states:

   ```bash
   scripts/buildctl migrate --from prod --to dev --prod-stopped
   ```

4. Keep prod stopped. Deploy the reviewed dev ref through buildctl, which installs
   the shared roots in the unit before starting dev. Replace `<validated-ref>`
   with the selected commit. Do not directly start the old dev unit:

   ```bash
   scripts/buildctl --env dev bridge deploy <validated-ref>
   scripts/buildctl --env dev bridge status
   scripts/buildctl --env dev bridge logs
   ```

   Open dev and check projects, tasks, attachments, workspaces and resumed agents.
   Harness session IDs and the resume roster are preserved. Pi sessions under
   the state root are copied; external Claude/Codex session stores are never
   copied or rewritten. Historical session IDs alone do not guarantee that a
   provider can resume a session; confirm important in-flight agents manually.

If publishing fails, the error reports the retained backup path. Keep that
backup and its sibling JSON manifest; the same rollback command recovers a
partially published migration and mixed old/new review remote values.

Missing roots referenced only by known terminal operations are listed in
`historical_operation_roots`; queued or uncertain operation roots are required.

To roll back, first have the operator stop dev and confirm both bridges are
inactive. Restore using the exact backup path printed by migration, then have
the operator restart prod:

```bash
systemctl --user stop build-bridge-dev.service
systemctl --user is-active build-bridge-dev.service
systemctl --user is-active build-bridge.service
scripts/buildctl migrate --from prod --to dev --rollback <backup-path>
systemctl --user start build-bridge.service
```

Rollback restores the previous dev state and shared Git URL changes; it keeps
the migrated state in the reported `.retired-…` directory and marks the backup
manifest `rolled_back`. It does not
merge work created after cutover back into prod. Inspect/export such work before
rollback. Leave dev stopped; before using restored dev state again, deploy its
intended ref through buildctl to refresh its unit environment. Never run both
bridges against the adopted repositories and worktrees at once.

## Hosted app and relay

```bash
scripts/buildctl --env dev app deploy <ref>
scripts/buildctl --env dev app deploy <ref> --force
scripts/buildctl --env dev app status
scripts/buildctl --env dev status
```

App deploy resolves a commit, fetches the environment's branch, checks ancestry,
and pushes that commit to the branch. A non-fast-forward is refused unless
`--force` is explicit; that path uses an exact `--force-with-lease` against the
fetched tip. Concurrent branch changes refuse the forced push. No tags,
workflow dispatches or other branches are touched. The target ref must include
the dev deployment workflow; moving `dev` triggers its deploy-only hosted flow
([dev deployment contract](../../deploy/k8s/dev/README.md#automatic-deployment-and-branch-ownership)).

App status reads `/app/static/version.json` and the latest deployment workflow
run with `gh` by workflow name. Dev's deploy workflow runs from `dev`. The reported public version and workflow run can differ
while a deployment is pending, failed, or superseded; status reports both rather
than asserting they are the same revision. Endpoint and workflow failures are
reported independently, preserving whichever status is available. The combined
status command includes both local bridge and hosted status.

## Offline gates

```bash
nice -n 10 uv sync --frozen --project tools/buildctl
nice -n 10 uv run --frozen --project tools/buildctl pytest tools/buildctl/tests -q
nice -n 10 uv run --frozen --project tools/buildctl ruff check tools/buildctl
nice -n 10 shellcheck scripts/buildctl
```

CI runs these in the `buildctl` job, required by both hosted image-build gates.
Tests use temporary homes and injected git/cargo/bridge/systemd runners, process
executable links and HTTP responses. They build no real bridge, touch no real
service, and make no network calls. Do not use a live environment to run tests.
