# Bridge updates

The device settings page reads a cached bridge status. Wire 1.18.0 adds
`bridge.update_status`, `bridge.check_update`, and
`bridge.install_update {"when":"now"|"idle"}`. Each returns the status; subsequent
changes arrive as an event with `type: "bridge.update_status"` and the status
fields at its top level.

Install commands persist their accepted state before replying. Check commands
return promptly and publish the completed check through the same event.
RPC replies update the browser cache only if its persisted generation still
matches the one read before the request. The comparison and write share one
IndexedDB transaction, so a delayed cross-tab notification cannot hide a newer
committed status.

The status contains `running_version`, `platform`, `development_build`,
`latest_release` (`version`, `tag`, nullable `published_at`), nullable
`last_checked_at`, `state`, nullable `last_error`, `update_available`, and
`can_install`. Timestamps use RFC 3339. States are `idle`, `available`,
`scheduled_when_idle`, `installing`, and `failed`. A failed install can still
have an available release. A successful metadata check does not clear the
previous installation error.

An idle installation waits for work already running or queued on this bridge.
Scheduling and downloading leave the device open to new work. At the final
idle handoff, the bridge atomically closes admission to new RPC, MCP, and queued
agent work until the helper stops it. A conclusive launch failure reopens
admission; an uncertain launch keeps the attempt reserved for recovery. Status
requests remain available. Scheduling survives daemon restart. Checks run
daily, with an explicit check available in settings.

## Installation boundary

Only an installer-managed executable with matching local provenance and a
matching background service can replace itself. A source build, an unmarked
older installation, or a service pointing elsewhere reports a development
build. It can check releases but cannot install them. Installing a signed
release with the updated installer establishes the supported path; merely
copying a source binary over an installed one does not.

Release metadata comes from the configured public releases repository
(`ZechCodes/build-releases` by default). Before stopping anything, the updater
downloads the platform archive, `SHA256SUMS`, and `SHA256SUMS.sigstore.json`.
Verification is mandatory. The updater pins cosign's version and executable
digest and checks the certificate issuer and the exact release workflow/tag
identity, then the archive digest and contents. There is no unsigned fallback.

The copied helper runs in its own systemd user job or launchd job. It survives
the bridge service stopping and owns binary/store backup, replacement, health
checking, and rollback. A launcher error does not prove the helper failed to
start: the active attempt remains reserved, and the running bridge retries
recovery until the helper records a terminal outcome. Helper and backup files,
their directory entries, and install-marker replacement are synced before the
next durable phase. Recovery consumes only validated backups published
atomically and covered by a durable backup-ready checkpoint. The helper checks
the staged digest again before executing the version probe and checks the exact
replacement bytes before installing.
Its outcome lives outside the restored store so a
rollback does not erase its error. The new daemon must prove startup readiness
with the expected version and attempt identity, then remain healthy for the
stabilization window. Its local heartbeat also requires responsive application
state and a live process. The candidate withholds its relay connection and agent
resumption until probation completes, preventing new work from entering a store
that might be restored. An unrelated relay outage does not fail this local
health check. A failed candidate restores the previous binary and store before
restarting the old service. Interrupted helpers resume recovery; a pending
rollback blocks another installation.

## Release and rollout

Merging the SPA alone does not enable self-updates on old bridges. Roll the
bridge after merging, and publish an updater-capable signed bridge release
using `.github/workflows/release.yml`. The crate version and `bridge-vX.Y.Z`
tag must match. The first installation uses the updated published installer;
later releases must have a greater semver than the running executable.

For alpha, publish an initial signed release containing this change, followed
by a greater patch version to demonstrate an actual remote update. If
`bridge-v0.2.0` has not been published, it can be the initial release and
`bridge-v0.2.1` the update test; if the tag already exists, choose two unused
successive versions and update `bridge/Cargo.toml` for each release. Do not
replace an existing tag or release asset to simulate a new version.

Before public releases exist, use the local release-server and helper tests.
They exercise download verification, replacement, rollback, persisted errors,
and scheduling without stopping the developer's bridge. The Rust/SPA wiring
test checks the real RPC, push event, cache, and settings renderer. Browser
fixtures provide screenshots of the settings states. The production release
check should report an error if the public repository has no latest release.

The normal Rust suite uses an offline signed local-release fixture. Additional
live Sigstore coverage is available with a downloaded cosign executable whose
bytes match the production pin:

```sh
cd bridge
BUILD_COSIGN_TEST_BINARY=/path/to/pinned/cosign cargo test --lib update::release::signature_tests -- --nocapture
```

The SPA suite invokes the Rust RPC probe itself; run the Rust gates first to
reuse their compiled test binary. The SPA CI job installs Rust and caches the
same target directory. No generated wire fixture or skipped test substitutes
for the probe.

After signed releases exist, use a disposable paired Linux user and a macOS
user to verify the native service managers: install the older release, open
device settings remotely, check, schedule while an agent works, let it become
idle, and confirm the new running version after reconnect. Repeat with Install
now. Exercise a signed candidate that cannot become healthy in a controlled
release repository, confirm binary and store restoration and the persistent
error, then install a healthy greater version and confirm the error clears.

Keep the machine's existing bridge and store out of this destructive rollout
test. The helper's unit tests isolate their service manager and filesystem;
they do not by themselves certify a native launchd run on Linux.
