# Secure transport consolidation

Planning deliverable for #572, 2026-10-12. Proposed migration, not shipped.

Zech authorized keeping the required first-party code in Build on
#348/c/tc-01M4BN9YTE943BXC71MTDARJEK. This plan carries that decision forward.
It does not request a new decision about repository ownership. The coordinator
dispatches implementation; #569 owns serial main integration. Astra assisted
planning; Sol implements the children, followed by independent review.

## Decision and boundary

Put the JS binding, Python reference, tests, package manifests, locks, protocol
document and license in `transport/`. Keep the Rust implementation at
`bridge/src/transport.rs`. It is already in-tree; extracting another crate would
unnecessarily change the bridge, relay image and release build boundaries.

Import source from `ZechCodes/build-secure-transport` commit
`4bce3aacebd66a9674aaa96c6ec0d97ab4bcd793`, the exact commit currently pinned by
`skriftapp/Containerfile`. Build was inspected at
`48f381ecf1a1b75465afa216d67465bfbd7e053e`; implementation must reconcile any
subsequent main changes before editing. The companion
[inventory and evidence ledger](Secure%20Transport%20Consolidation%20Evidence.md)
records the source mapping and verification obligations.

```text
transport/
  README.md                    # ownership, current topology, development
  UPSTREAM.md                  # source SHA, import map, hashes, exclusions
  LICENSE.md                   # preserved transport license
  E2E-MVP.md                   # historical protocol document with context
  fixtures/                   # non-secret pre-move compatibility vectors
  js/{package.json,package-lock.json,README.md,src/,test/}
  python/{pyproject.toml,uv.lock,README.md,src/build_secure_transport/,tests/}
bridge/src/transport.rs         # existing Rust binding remains here
```

`@build/secure-transport` and `build_secure_transport` retain their names,
exports and initial package versions. SPA and web resolve
`file:../transport/js`. Python is a reference and test dependency; do not add it
to the production Skrift application or bridge runtime. Third-party packages
remain normal dependencies. A single checkout removes the first-party sibling
requirement; it does not promise an offline dependency install.

The first import preserves crypto source bytes. Test launchers, package metadata
and documentation may change in separately visible commits. No key rotation,
protocol/API version bump, session change, relay behavior, network policy,
identity migration or live bridge/client action belongs in this migration.

## Source and licensing treatment

Import through an explicit tracked-file allowlist at the pinned commit, never a
recursive working-directory copy. Upstream tracks redundant Python `build/lib`
and `*.egg-info` outputs. Its checkout also contains an untracked recursive
symlink. Neither those nor dependency directories, environments, `.git` or
`.rift` enter Build. Record each imported path and SHA-256 in `UPSTREAM.md`;
record source documentation edits separately from the unmodified baseline.

Build's root license is AGPL-3.0-only. The transport's `LICENSE.md` is Business
Source License 1.1 with licensor Zechariah Zimmerman, a four-year change period
and MIT change license. Preserve that file and an explicit scoped exception in
Build's README. Moving source does not itself relicense it. Do not overwrite the
transport notice with the root license or imply its change date has elapsed.
The existing CLA is not evidence that every historical contribution was
relicensed. This is a record of current notices, not a new legal conclusion.

The package manifests currently omit license metadata. #575 must make the
scoped notice visible in locally built npm/Python artifacts and metadata without
changing its terms. Verify the resulting tarball/wheel/sdist file lists. Do not
publish them. Any proposed relicensing is separate from this move; it must name
the exact scope and rights basis before a later change claims it.

## Sequenced child tasks

All children are filed under #572 in Backlog, not dispatched by this planning
pass. The task bodies contain implementation and acceptance details. These are
dependency order, not permission to run several broad suites concurrently.

| Order | Task | Depends on | Reviewable result |
| --- | --- | --- | --- |
| 1 | #575 Import sources, provenance and scoped licensing | reviewed plan | Byte-identical canonical source import, exclusions, hashes, locally verified license/package contents |
| 2 | #576 Mandatory portable compatibility harness | #575 | In-tree Python discovery, portable JS launcher, frozen resolution, old/new vectors, no passing skips |
| 3 | #577 In-tree SPA/web dependencies and locks | #575, #576 | Both relative dependencies switched, portable committed locks, documented Node/npm and install order |
| 4 | #578 Containers, CI and change routing | #575–#577 | No external source clones, root QA context, required transport checks and tier regressions |
| 5 | #579 Ownership, contributor and legacy-transition docs | #575–#578 | Current one-checkout instructions, local audit/reporting links, explicit license scope, legacy pointer draft |
| 6 | #580 Independent review and integration handoff | #575–#579 | Exact-head verdict and complete evidence for coordinator/#569 |

Use separate commits per child. Treat #577 and #578 as one integration cohort:
do not land the consumer path change before the app image, QA context and CI can
resolve it. Prefer one reviewed final cohort through #569; #575/#576 can be
reviewed early but are not an excuse to run unneeded main deployments. No new
child is assigned or implemented by #572.

## Package and build migration

Install the local JS package's own dependencies before consumer installs; local
package links must not depend on an old sibling's `node_modules`. Preserve its
locked libsodium/libsodium-wrappers 0.7.16 initially. The SPA's Vite shim resolves
libsodium from `spa/node_modules`; verify Node and browser/Vite resolution after
the move rather than assuming identical dependency placement.

Commit portable `spa/package-lock.json` and `web/package-lock.json` after moving
the file dependency, alongside the retained transport JS and Python locks.
Remove their ignore entries and change the existing `AGENTS.md` prohibition on
SPA `npm ci` in the same implementation. Establish a tested Node/npm pair across
local setup, CI and images. The current image uses Node 22 and npm 12.0.2 because
of an npm 10 peer-resolution crash; preserve this constraint until evidence
supports changing it. Keep SPA `--legacy-peer-deps` consistent at lock creation
and frozen install. Reject unrelated dependency upgrades or absolute lock paths.
Until #577 lands, existing repository install instructions remain in force.

Change `skriftapp/Containerfile` to copy `transport/js` beside SPA under the same
repository layout, install from locks, and remove its Git clone and
`BUILD_SECURE_TRANSPORT_COMMIT` argument. The Build commit becomes the code pin;
`UPSTREAM.md` retains the original source pin for audit. Preserve relay/version
build arguments, static output and stage boundaries.

Change the QA build in `deploy/compose.real.yml` from context `../web` to `..`,
recipe `web/Containerfile`. Copy only web sources/manifests and transport JS into
matching sibling directories. Remove the external clone and `npm pkg set`
absolute-path rewrite. Check `serve.mjs` paths and QA working directory. Keep
`--omit=dev` behavior where applicable; do not execute QA against live services.

Update **both** root `.dockerignore` and `.containerignore`: include committed
consumer locks and exclude transport node_modules, Python environments, build
outputs, caches and secrets. `web/.dockerignore` no longer governs a root-context
build. Narrow COPY instructions must prevent unrelated repository/private files
from entering the final images. Audit docs/scripts that invoke the old context.

## Compatibility and CI contract

Preserve transport version 1, X25519 sealed-box bootstrap, 32-byte session keys,
24-byte random secretbox nonces, Ed25519 transport-key signatures, identity
fingerprints, standard unpadded Base64 output and existing input acceptance.
Keep outer envelope fields and encrypted data/close frame fields unchanged,
including inner/outer session-id checks and session-accept verification. These
are distinct from Build's RPC wire version and capabilities.

The libraries do not become replay filters or authenticate `route_to` merely
because they move. Current architecture supplies API-owned presence/device keys
and a signaling-only relay with WebRTC data channels. Historical `E2E-MVP.md`
describes an older topology; annotate that distinction, do not change crypto to
make it match. Record existing cross-language parser/serialization differences
before proposing any behavior change in another task.

The required transport job runs Python unit tests, all JS tests (including
JS/Python interop), Rust transport tests and Rust/Python interop. There is no
existing direct JS/Rust suite: describe evidence accurately. Freeze dependencies
and use a portable explicit interpreter strategy, not `<root>/.venv/bin/python`.
Resolve Rust's default reference from its manifest directory; an explicit test
override can remain, but absent tools/source must fail the required test.

Generate fixed non-secret vectors from the pinned pre-move implementation, with
fixed nonce, key, message id and timestamp where APIs support them. Sealed boxes
are randomized: keep a captured valid fixture and test decryption, do not expect
fresh wraps to match bytes. Exercise imported and pre-move implementations in
both directions during implementation, then retain immutable fixtures and hashes
so ongoing CI needs no external checkout. Preserve signature/fingerprint,
bootstrap/accept, data/close, Unicode/nested payload and rejection behavior.
Tests exposing pre-existing differences must distinguish them from regressions.

Add a `transport` check output to the selector and workflow. Keep selection of
checks distinct from selection of images to deploy:

| Changed input | Checks required | Image selection |
| --- | --- | --- |
| `transport/js` source or dependency manifests/lock | transport, SPA, E2E | app; not relay |
| Python source/manifests/lock, transport tests/shared fixtures | transport | neither solely for these paths |
| Rust transport or interop harness | transport plus existing bridge checks | existing relay-source rules (transport.rs is a relay source) |
| Transport runner, selector or CI workflow | transport and selector/gate regressions | preserve existing image rules |
| Only transport documentation/notices | link/license review; package checks if artifact content changes | no forced relay rollout |

The current selector bases owed work on deployed image commits; a Python-only
change deploys nothing. Use a separate conservative transport baseline from the
oldest relevant known deployed commit, and force checks when history is unknown.
Do not mark such evidence current merely because the relay has no source
changes. Repeating small transport checks after Python-only changes is acceptable
until a durable successful-check baseline is designed. Test a canceled/failed
transport change followed by an app-only change: the later app may not forgive
the missing compatibility evidence. `build-app` and `build-relay` must require
transport success whenever owed, with skipped/canceled/failed jobs not accepted.
Existing build-app bridge/interop guards remain effective.

## Review, integration and rollback

Each behavior-changing test/build harness child uses TDD and scoped checks. The
final #580 review names the exact feature SHA and completes the companion ledger
from evidence, not from a numeric claim. #569 rechecks current main, runs the
established candidate gates plus new mandatory transport/tier/package checks,
records actual exit codes and scans, and integrates serially after approval.
Changed heads require renewed review. The coordinator owns dispatch and timing;
this document does not dispatch CI, publish, tag, deploy or restart anything.

A move-only regression should be reversible by reverting the cutover cohort
together, restoring the old pinned production transport reference and consumer
paths. Keep the legacy repository and commit reachable for that purpose. No
database or identity rollback is needed because none are changed. If a source or
dependency difference is discovered, stop integration and resolve it visibly;
do not treat a passing same-language round trip as proof of compatibility.

After verified landing, Build is the maintenance/audit source of truth. #579
prepares a legacy README pointer with the actual migration commit and Build's
security reporting destination; applying it is a separately coordinated action.
Do not delete/archive the old repo, rewrite history, remove releases, or publish
npm/PyPI packages as part of this task. Historical links remain valid provenance.

## Planning validation

#572 inspects tracked source, manifests, license text, existing tests and CI. It
runs only documentation/static consistency checks. No dependency installation,
crypto/runtime test, container build, full gate, live pairing or deployment is
part of this pass. All runtime acceptance evidence remains explicitly pending in
the companion ledger and implementation children.
