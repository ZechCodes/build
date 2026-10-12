# Secure transport inventory and evidence

Companion to [the #572 migration plan](Secure%20Transport%20Consolidation%20Plan.md).
Inspection date: 2026-10-12. The original #572 planning inventory is retained;
#575 local receipts below cover import/package evidence P1/P2; #576 receipts
cover compatibility evidence C1–C3; #577 receipts cover consumer evidence
D1–D2. The remaining migration evidence is pending.
Paths below are relative to Build unless prefixed `upstream:`.

## Inspected baseline

| Source | Observed value |
| --- | --- |
| Build planning branch | `build/transport-consolidation` |
| Build source baseline | `48f381ecf1a1b75465afa216d67465bfbd7e053e` |
| Sibling transport commit | `4bce3aacebd66a9674aaa96c6ec0d97ab4bcd793` |
| Production recipe transport pin | same commit, `skriftapp/Containerfile:29` |
| Local sibling checkout | `/home/zech/Projects/.build-worktrees/proj-1/build-secure-transport` (inspection only; not a future dependency) |
| Source state | tracked tree clean; untracked recursive `build-secure-transport` symlink excluded |
| Source CI | no tracked `.github` workflow; Build runs its Rust/Python interop |
| JS package | `@build/secure-transport` 0.1.0, ESM export `./src/index.js`, `node --test` |
| JS dependency | range `libsodium-wrappers ^0.7.15`; committed lock resolves wrapper and libsodium to 0.7.16 |
| Python package | `build-secure-transport` 0.1.0, setuptools, Python >=3.10, PyNaCl >=1.5,<2 |
| Python lock | PyNaCl 1.6.2, cffi 2.0.0, pycparser 3.0; editable local package |
| Rust implementation | bridge package 0.2.12, dryoc 0.7, transport protocol version 1 |
| License records | root `LICENSE`/bridge manifest AGPL-3.0-only; upstream `LICENSE.md` BSL with MIT change license |

## Import mapping (#575)

| Pinned upstream files | Planned destination | Treatment |
| --- | --- | --- |
| `js/src/index.js` | `transport/js/src/index.js` | preserve executable bytes |
| `js/test/index.test.js` | `transport/js/test/index.test.js` | import baseline, separately adapt launcher in #576 |
| `js/package.json`, `js/package-lock.json`, `js/README.md` | corresponding `transport/js/` paths | retain name/version/dependency baseline; record metadata/doc changes |
| `python/src/build_secure_transport/{__init__,_protocol,device,exceptions}.py` | corresponding `transport/python/src/` paths | preserve executable bytes |
| `python/tests/{interop_helper.py,test_device.py}` | corresponding `transport/python/tests/` paths | preserve baseline; any harness edits separately reviewed |
| `python/{pyproject.toml,uv.lock,README.md}` | corresponding `transport/python/` paths | retain reference package and lock; document local packaging |
| `LICENSE.md`, `E2E-MVP.md`, `README.md` | corresponding `transport/` paths | license unchanged; record historical-doc corrections |
| `.gitignore` | explicit transport/root ignores as needed | review entries; do not blindly replace Build ignores |
| `python/build/lib/build_secure_transport/*.py` | excluded | tracked generated source copies; canonical source is under `python/src` |
| `python/src/build_secure_transport.egg-info/*` | excluded | tracked packaging output; regenerate locally when needed |
| `.git`, `.rift`, environments, dependency trees, recursive symlink | excluded | not canonical source, regardless of local presence |

Implementation must save the full imported file/hash manifest, not only this
directory map. Keep the initial source copy distinguishable from later edits.
Do not follow symlinks while enumerating the import.

## Consumer and packaging inventory

| Surface | Current coupling | Child |
| --- | --- | --- |
| `spa/package.json:23`, `web/package.json:16` | `file:../../build-secure-transport/js` | #577 |
| SPA `connection.js`, terminal `manager.js`; web harness imports | stable package name imports; preserve | #577 |
| `spa/vite.config.js:5` | wrapper ESM shim points at SPA-local libsodium | #577 |
| `.gitignore` | ignores SPA/web package locks as machine-specific | #577 |
| `skriftapp/Containerfile` | clones pinned sibling, npm 12.0.2 on Node 22; npm install | #578 |
| `web/Containerfile` | clones unpinned upstream HEAD, rewrites dependency to absolute path | #578 |
| `deploy/compose.real.yml:116` | QA context is web-only, cannot COPY a sibling within Build | #578 |
| `.dockerignore`, `.containerignore` | identical root patterns ignore SPA lock; lack new transport environment exclusions | #578 |
| `web/.dockerignore` | old web-context exclusions; audit when moving to root context | #578 |
| `.github/workflows/ci.yml` interop | unpinned clone `/tmp/bst`; env-gated Rust test | #578 |
| same workflow, SPA job | unpinned sibling clone + package install; no committed SPA lock/cache | #578 |
| `.github/changed-tiers.sh` | app/bridge/E2E patterns do not include new transport tree | #578 |
| `.github/test-changed-tiers.sh` | synthetic history regression tests; add dependency and canceled-check cases | #578 |
| `.github/relay-sources` | includes Rust `bridge/src/transport.rs`; preserve accurate narrow relay inputs | #578 |
| `skriftapp/pyproject.toml` | no Python secure-transport runtime dependency; pairing_crypto uses cryptography | unchanged |
| `bridge/Cargo.toml`, bridge release workflow/Containerfile | Rust already in-tree; no extraction needed | unchanged unless test wiring requires it |
| desktop | loads hosted SPA; no transport package import or standalone bundling | no desktop migration needed |

## Existing compatibility coverage

| Test source | What exists at baseline | Gap relevant to relocation |
| --- | --- | --- |
| upstream JS `test/index.test.js` | four tests: identity helpers, Python fingerprint comparison, JS round trip, JS/Python bootstrap and bidirectional messaging | assumes transport-root `.venv/bin/python`; currently not explicitly run by Build CI |
| upstream Python `tests/test_device.py` | five unittest cases: signed key round trip/tamper, bootstrap/message/close, session mismatch, ciphertext tamper | needs an explicit in-tree CI invocation |
| `bridge/src/transport.rs` tests | key sizes, identity seed, signatures, fingerprint known answer, bootstrap/accept/frame round trips, wrong key, explicit nonce determinism, bad version, signature-input rejection | preserve and run; not a direct JS/Rust exchange |
| `bridge/tests/interop_python.rs` | Python signed keys/fingerprint, Rust sealed box opened by Python, Python accept in Rust, messages both ways, Rust close in Python | returns success without exercising anything if `BUILD_SECURE_TRANSPORT_PY` is unset; uv call is not frozen |

JS/Python envelope validators default a missing version to 1; Rust's typed
`Envelope` requires a version during deserialization. Record this as existing
behavior; do not silently normalize it during migration. Similarly, JSON
number/string serialization and malformed Base64 acceptance need baseline
fixtures rather than assumptions of universal byte identity. Runtime evidence
was not collected in #572.

## Documentation/reference audit (#579)

Current operational references to update:

- Root `README.md` architecture and licensing paragraphs; `SECURITY.md` currently
  sends transport vulnerabilities to the separate repository.
- `CONTRIBUTING.md` sibling-clone setup, missing-binding warning, interop command;
  `AGENTS.md` install rules and gate prerequisites.
- `ARCHITECTURE.md` topology table, test inventory and SPA dependency explanation.
- `spa/README.md`, `web/README.md`, deployment/build context instructions, and
  `spa/.claude/skills/verify/SKILL.md` sibling-symlink setup.
- `planning/v2/Direct Connection Spec.html`, `WebRTC Transport Spec.md`,
  `Test Suite Audit.md`, `roadmap.md`: update active paths or label historical
  references; imported E2E-MVP topology must not supersede current architecture.
- `CHANGELOG.md` migration note and transport README audit/development pointers.

Keep deliberate provenance links and harmless historical fixtures (for example
the repository name in `spa/test/browser/captureRepoPicker.mjs`) classified,
rather than mechanically rewriting every occurrence. Search tracked hidden
files as well as ordinary source files. External clone instructions and active
out-of-tree paths must have no surviving operational use.

## Acceptance evidence ledger (implementation in progress)

Each item must name exact commit, command/environment, exit status and attached
log or artifact when completed. No row is marked passed from source inspection.
These are the migration security requirements; all must be satisfied before
reporting the implementation's security checklist complete.

| ID | Required evidence | Owner | Planning status |
| --- | --- | --- | --- |
| P1 | Imported canonical file hashes match pinned source; exclusions verified | #575 | locally verified; #575 receipts below |
| P2 | Scoped BSL notice retained; npm tarball and Python sdist/wheel include correct source/license; no generated duplicate sources | #575 | locally verified; #575 receipts below |
| C1 | Python 5-case and JS 4-test baselines retained/passing; later added cases separately counted | #576 | locally verified; #576 receipts below |
| C2 | Rust transport tests and actual Rust/Python + JS/Python exchanges pass; missing prerequisite fails | #576 | locally verified; #576 receipts below |
| C3 | Non-secret old/new vectors cover identity/bootstrap/accept/data/close and malformed inputs, Unicode/nested payloads; no nonce reuse introduced | #576 | locally verified; #576 receipts below |
| D1 | Independent clean checkout paths install from locks and resolve in-tree source without sibling symlinks or old dependencies | #577 | locally verified; #577 receipts below |
| D2 | Node and Vite libsodium resolution; SPA lint/focused tests/build; web unit tests | #577 | locally verified; #577 receipts below |
| B1 | App and QA images build from clean repository context without external transport access; final contents/notices reviewed | #578 | locally verified; #578 receipts below |
| B2 | Root Docker/Podman ignores and narrow COPY rules exclude environments/private files and retain necessary locks | #578 | locally verified; #578 receipts below |
| G1 | JS-only, Python-only, Rust, fixture, workflow/selector and unrelated-doc path cases select intended checks/images | #578 | locally verified; #578 receipts below |
| G2 | Failed/skipped/canceled/missing compatibility evidence blocks owed image builds, including a later app-only commit | #578 | locally verified; #578 receipts below |
| O1 | One-checkout docs, reporting destination and remaining historical references reviewed; old repo untouched | #579 | pending |
| R1 | Independent approval names exact feature SHA and resolves findings | #580 | pending |
| R2 | #569 records final candidate/base, required full gates plus transport checks, scans and actual integration outcome | coordinator/#569 | pending |

Suggested future focused entrypoints, after #576 defines the launcher contract:
Python `uv run --frozen --project transport/python python -m unittest discover
-s transport/python/tests`; JS `npm test --prefix transport/js`; from `bridge/`,
`cargo test transport::tests` and `cargo test --test interop_python -- --nocapture`.
Run these under `nice -n 10` and capture exit codes. The JS launcher and Rust
reference default must be repaired before these count as complete evidence.
Do not run these commands during #572's planning-only pass.

Final implementation validation adds existing candidate gates (bridge tests,
clippy, fmt and vendored tests; SPA lint, full test/capture/build; relevant
packaging, shell/tier and security scans). Coordinate their timing through #569
instead of launching full suites from each planning or review conversation.

## #575 local import/package receipts (2026-10-12)

At the #575 checkpoint only P1/P2 were verified; C1–R2 remained pending. These
import/package receipts are not runtime compatibility evidence or the final
migration security checklist/integration. #576 compatibility receipts follow.

- Base: `215bc79c7e91c4319f10b0c7f9dd2969030df794` after fetching origin/main
  and fast-forwarding the clean managed `build/transport-import` branch.
- Initial import: `496716692caa3a711a7f627952d711adfa52ac73`.
- Package/code validation tree: `20ccc2edcaecba6a2c640013cef98bf3502a10a7`.
  Later receipt/root-scope prose changes do not change any package/checker bytes.
- Pin: `4bce3aacebd66a9674aaa96c6ec0d97ab4bcd793`; 17 original SHA-256
  digests checked, with direct byte comparison of all unamended files. The
  current digests and five separately recorded doc/metadata amendments are in
  `transport/UPSTREAM.md`. Ten tracked exclusions comprise upstream `.gitignore`
  and nine generated Python outputs. The sibling remains at that pin with only
  its pre-existing untracked recursive symlink; no source writes were made.
- Environment: Linux x86_64; stdlib checks Python 3.14.7; npm 11.19.0 on Node
  26.8.1; uv 0.12.13 auto-selected Python 3.13.15 for packaging; wheel reports
  setuptools 84.0.0. Consumer/CI Node/npm policy is unchanged by this child.
- Granted #569 packaging slot: serial jobs at nice 10, uv concurrent builds 1;
  no Rust, SPA, browser or full suite. Artifacts are from `git archive` of the
  committed validation tree, in disposable `/tmp/transport-575-receipts/final/`.
  No package publication or live service action occurred.

| Receipt | Command (from Build root unless noted) | PID | Exit/result |
| --- | --- | --- | --- |
| import-red.log | `nice -n 10 python -m unittest discover -s transport/tests -v` before import/checker | 3871380 | 1, required import absent |
| import-green.log | same, after initial import | 3873366 | 0, five cases |
| package-red.log | `nice -n 10 python -m unittest discover -s transport/tests -p test_package_contract.py -v` before license amendment | 3875378 | 1, notices/metadata absent |
| artifact-red.log | `nice -n 10 python scripts/transport_packages.py --npm <baseline.tgz> --sdist <baseline.tar.gz> --wheel <baseline.whl>` | 3880556 | 1, missing npm notice and unwanted test member |
| checks-green.log | `nice -n 10 python -m unittest discover -s transport/tests -v` | 3900184 | 0, 15 import/metadata/archive cases |
| import-final.log | `nice -n 10 python scripts/transport_import.py check --upstream <read-only-sibling>` | 3949864 | 0, pin/digests/bytes/exclusions |
| npm-pack.log | `nice -n 10 npm pack <export>/js --ignore-scripts --json --pack-destination <artifacts>` | 3902218 | 0 |
| python-build.log | `UV_CONCURRENT_BUILDS=1 nice -n 10 uv build <export>/python --out-dir <artifacts>` | 3902357 | 0, sdist then wheel from sdist |
| artifact-check.log | `nice -n 10 python scripts/transport_packages.py --npm <tgz> --sdist <tar.gz> --wheel <whl>` | 3906213 | 0, 4/18/9 file members checked |
| semgrep-all.log | `nice -n 10 semgrep --config auto --error --jobs 1 --x-ignore-semgrepignore-files <13 changed executable/test files>` | 3929472 | 0, 442 rules, no findings |
| gitleaks.log | `nice -n 10 gitleaks git --no-banner --redact --log-opts='215bc79c7e91c4319f10b0c7f9dd2969030df794..HEAD' .` | 3924172 | 0, no leaks |

The default Semgrep pass also exited 0 but excluded five tests via repository
ignore rules; the recorded all-files pass explicitly included them. Import and
archive checks reject source/license drift, unexpected generated source,
missing canonical files, misleading license metadata, duplicate archive paths,
traversal and symlink members. Archive members are read without extraction.
The sdist's generated egg-info contains only metadata; no `build/lib` source
copies, dependency tree, environment or caches appear in any artifact.

Artifact SHA-256 (file listings are in attached `artifact-check.log` on #575):

| Local artifact | SHA-256 |
| --- | --- |
| `build-secure-transport-0.1.0.tgz` | `56e6a765e90359d5e775167b1c5a428dc52320e383e70b0d6eb38c8004438a1e` |
| `build_secure_transport-0.1.0.tar.gz` | `19350afd5b6842120c913373d47674e82f2618e4ead5b6c932f63bfdded83e2e` |
| `build_secure_transport-0.1.0-py3-none-any.whl` | `78c1b22366a849d673a9b19bfc63d1a726e6c2a12ed27bd57337c212bb24b430` |

The preserved setuptools >=69 requirement supports file-based license metadata.
Setuptools 84 emits deprecation warnings recommending its >=77 SPDX-expression
metadata interface before February 2027. The local artifacts include the full
unchanged notice in metadata and as a license file today; a future packaging
backend/metadata modernization must preserve the custom terms. No runtime
requirement or lock resolution was upgraded to suppress that warning.

## #576 local compatibility receipts (2026-10-12)

C1–C3 are locally verified. D1–R2 remain pending for their assigned children;
this is not final migration approval or integration. Full logs and command,
head/base, PID, environment and exit receipts are attached on #576.

- Base: `99eaab748857a552a5fc30384ba848c5d1e47f13`.
- JS/Python and old/new directional receipts are frozen at
  `1f7db8234664b3425b6d5116566dbe50cfb16659` on
  #576/c/tc-01M4RYEJ8G9JPVHKKVF7FCFY61. Their source and dependency bytes
  remain unchanged by the Rust completion.
- Completed Rust source: `4445f2203d8613fe54c07fdd258876dff0220b57`.
  The final receipt/scan amendments change only this evidence document and
  two exact historical public-fixture fingerprints in `.gitleaksignore`; the
  attached equality proof binds the review head to this validated source tree.
- Gates ran serially under `nice -n 10`, with two Cargo build jobs. Final Rust
  runs used two test threads; the initial red/first full run used one. uv is the
  already-installed 0.12.13 selected explicitly through `BST_UV`; Python runs
  and both subprocess launchers use frozen resolution.
- Rust discovers `transport/python` from `CARGO_MANIFEST_DIR`, validates the
  required reference files, and exercises the existing bidirectional exchange.
  A deliberate reference override remains available. Missing reference or uv
  fails the actual round-trip test clearly, with no passing skip.

| Receipt | Command / scope | PID | Exit / result |
| --- | --- | --- | --- |
| rust-red | `cargo test --no-run -j2` at the checkpoint | 450216 | 101; four undefined-helper errors |
| rust-vectors-red | `cargo test --locked -j2 --test transport_vectors -- --nocapture` | 1245075 | 101; incorrect padded-Base64 baseline expectation |
| cargo-clippy | `cargo clippy -j2 --all-targets -- -D warnings` before splitting vector checks | 1650479 | 101; existing vector test scored 16/15 |
| cargo-test-r3 | `cargo test -j2` | 1729698 | 0; full suite, including 16 transport units, Rust/Python 3 and vectors 3 |
| cargo-clippy-r2 | `cargo clippy -j2 --all-targets -- -D warnings` | 2107078 | 0; no new ratchet annotation |
| cargo-fmt | `cargo fmt --check` | 2111708 | 0 |
| vendor-test | `cargo test --locked -j2 --manifest-path vendor/Cargo.toml --lib` | 2114340 | 0 |
| isolated-rust | root invocation with `--manifest-path bridge/Cargo.toml --test interop_python --test transport_vectors` | 2153014 | 0; six tests, default in-tree reference |
| isolated-missing-reference / isolated-missing-tool | actual required Rust round trip with deliberately absent reference / uv | 2241862 / 2241984 | 101 / 101; clear prerequisite errors |
| isolated-uv-sync / isolated-npm-ci | frozen uv sync / `npm ci --prefix transport/js --ignore-scripts` | 2242137 / 2242237 | 0 / 0 |
| isolated-python-five / isolated-python-vectors | frozen unittest discovery | 2242502 / 2242717 | 0 / 0; original five plus four added vector cases |
| isolated-js-eight | `npm test --prefix transport/js -- --test-concurrency=2` | 2242778 | 0; original four plus four added vector cases, including JS/Python |
| isolated-contracts | stdlib unittest discovery under `transport/tests` | 2243341 | 0; 19 import/package/launcher contracts |

The supplied unrelated checkout at
`/tmp/transport-576-receipts/isolated-checkout/Build` was clean at the completed
source head and required no sibling source. Cargo reused a build cache, then
recompiled the interop harness for that checkout's manifest directory. The
Python/JS installs and reference source are local to the isolated checkout.

Pinned old-source consumers accept the stored fixtures, and both old-source
consumers also accept newly generated vectors. Deterministic fields match the
pin in both directions. Randomized sealed wraps are excluded from producer
equality and verified by decryption. Distinct plaintext/producer variants use
distinct fixed nonces. Fixture bytes and their digest remain unchanged.

Rust's existing decoder accepts padded Base64 by stripping trailing `=`;
the first Rust vector run exposed the checkpoint's incorrect rejection claim.
The test and baseline table now record that acceptance, alongside the existing
version/JSON differences. Crypto/runtime sources, original Python helper/tests,
manifests and locks remain byte-identical to base. There is no direct JS/Rust
runtime exchange claim. #577 consumer policy, #578 containers/CI, #579 operational
documentation and #580 independent review remain outside this child's scope.

The initial final-head Gitleaks run exited 1 on two generic-key detections in
the fixture's introducing commit: captured sealed-box ciphertext (line 308) and
the deliberately public session key bytes 64..95 (line 310). The redacted
finding report is retained. Only those two commit/file/line fingerprints are
excluded; no fixture path or rule is suppressed. Fixture bytes stay immutable,
and complexity ratchet annotations remain unchanged. Final scans and their
exact-head logs are attached to the task handoff.


## #577 local consumer/lock receipts (2026-10-12)

D1–D2 are locally verified. Container/CI, operational documentation, independent
review and integration evidence B1–R2 remain with their assigned children.
This consumer cutover must integrate with #578's container/CI cutover.

- Base: `6335adbe0aeab4b60cb6bdf16cfb790321d44351`.
- Dependency cutover commit: `bab587d279f47f38c3eb16148d9b361e16fb0191`.
  Final policy/test source: `d2e1c1df8bd41a770059b1a7c28f0d20d92dfc5e`.
  The subsequent evidence-only commit leaves all executable/config/lock bytes
  and file modes unchanged; the final #577 handoff attaches that equality proof.
- Tested tools: Node 22.23.3, npm 12.0.2, Chromium `/usr/bin/chromium`.
  npm was installed only into a disposable tool directory with scripts disabled;
  no global tool or host configuration was changed. All gates ran at nice 10.
- Both consumers now resolve `file:../transport/js`. Their new committed locks
  contain registry URLs and the relative in-tree link, with no absolute or
  external transport paths. Dependency ranges/overrides retain main's values.
  A fresh, scripts-disabled lock-only resolution of main's manifests in a
  disposable fixture layout matches every registry version, resolution and
  integrity in the new consumer locks. This compares contemporaneous main
  resolution, not an untracked historical developer environment.
- Transport source/manifests/locks, Rust crypto, the Vite shim and both complexity
  ratchet tests remain byte-identical to base. SPA libsodium and wrappers match
  transport's retained 0.7.16 version/resolution/integrity exactly. Node initializes
  sodium from the linked package's own installed dependencies; Vite bundles the
  in-tree binding through the existing SPA-local sodium shim.
- SPA/web `.nvmrc`, `engines` and `packageManager` record the tested tools.
  `.npmrc` disables install scripts in each consumer, requires seven days
  before resolving a new registry release, and persists SPA's
  `legacy-peer-deps=true`. `AGENTS.md` changes the lockfile prohibition atomically
  with the portable locks. From the root, install transport first with
  `npm ci --prefix transport/js --ignore-scripts`; then use SPA
  `npm ci --legacy-peer-deps` or web `npm ci`. For dependency edits use SPA
  `npm install --legacy-peer-deps` or web `npm install`, committing the lock.

| Receipt | Command/scope | Exit/result |
| --- | --- | --- |
| consumer-red | stdlib consumer dependency contracts before cutover | 1; old paths, missing locks/policy and ignored locks |
| node-resolution-red | `node --test web/transportDependency.test.mjs` before install | 1; binding unavailable |
| transport-ci | `npm ci --prefix transport/js --ignore-scripts` | 0; retained lock |
| spa-install / web-install | `npm install --legacy-peer-deps` / `npm install` in consumer | 0 / 0; scripts disabled |
| consumer-green / contracts-all | consumer contracts / all transport contracts | 0 / 0; five / 24 cases |
| spa-focused / spa-lint / web-test | focused in-tree/sodium test / lint / unit tests | 0 / 0 / 0; one / clean / 23 cases |
| import-check / source-proof / baseline-graph | import digests / immutable sources and ranges / main resolution comparison | 0 / 0 / 0 |
| first-* / second-* (both rounds) | fresh checkout frozen installs, contracts, focused SPA, web unit tests, Vite build and clean tree | all 0 |
| release-age-red / release-age-green | consumer policy contract before/after seven-day release threshold | 1 / 0 |
| spa-install-r2 | `npm install --legacy-peer-deps` after policy amendment | 0; committed lock unchanged |
| initial/semgrep | initial scan requiring minimum release age in both new `.npmrc` files | 1; fixed by tested policy amendment |

The final two unrelated Git checkouts are
`/tmp/build-577-first-l7ou5_85/checkout` and
`/var/tmp/build-577-second-ku2ay9pe/checkout`, each with its own initially empty
npm cache and no prior dependency tree or external sibling source. Git objects
are shared solely for checkout creation; dependency installs fetch registry
artifacts independently. Transport dependencies are installed first, then SPA
with `--legacy-peer-deps`, then web. Both checkouts resolve the exact local source,
initialize sodium, pass web's 23 unit cases and Vite builds, and leave tracked
files unchanged. The initial round at the dependency cutover commit also passed;
its logs remain under `initial/` in the receipt bundle. After Semgrep found the
missing minimum release age, the full isolated-install sequence was repeated at
the final policy source commit. Both rounds used separate empty caches. No cache
or sibling-link prerequisite remains.

The final #577 task comment attaches the complete command/head/exit manifest,
logs for every required full SPA/capture/build and security gate, and final-head
source equality. These local D1/D2 receipts do not claim container builds, CI
routing, live QA, bridge execution, pairing, network changes, publication or
migration-wide approval. #578 owns container/CI wiring; #579 owns the remaining
operational/reference audit.

## #578 local container/CI receipts (2026-10-12)

The delivery branch starts from #577's `0e5636e44864d9765c2abcbe2d0c267220cadc94`.
Commit `5bb2295a` retains #607's uv 0.12.13 setup because the consumer branch
predates that fix. Delivery/routing implementation is through `ade97da4`;
this evidence update does not change runtime sources or dependencies.

App and QA recipes copy the checkout's transport JS manifests, lock, source and
notice beside the consumer, install transport first with scripts suppressed,
then install the committed consumer lock. Node 22.23.3/npm 12.0.2 match #577;
SPA retains `--legacy-peer-deps`, QA retains `--omit=dev`. QA now builds from the
root and runs in `/app/web`. Both root ignore files retain consumer locks and
exclude transport dependencies, Python environments/generated outputs and env
files. The app's landing install also suppresses lifecycle scripts. The app
runtime carries the exact transport BSL notice in `licenses/` beside its bundle;
QA retains that notice with the local package.

The dedicated transport job checks import/package/harness contracts before
installing dependencies, then Python units and vectors, full JS tests including
Python interop, Rust transport units/vectors and the Rust/Python round trip.
Existing bridge tests retain pinned uv; the interop job uses its default in-tree
reference without a clone or override. SPA and web jobs install transport first
and use frozen consumers. Image guards require transport success whenever owed;
missing, skipped, failed or canceled results cannot certify that evidence.
All existing bridge/interop image guards remain.

Transport diffs use the older actual app/relay deployment commit without the
bridge selector's unchanged-relay forgiveness. Python-only checks can therefore
repeat until both deployment tags advance; this conservative replay is deliberate.
They do not select either image by themselves. JS source/dependency changes select
app, E2E and transport, while relay selection still follows relay-sources only.
Rust manifests/exports, fixtures, workflow/selector/checker files and delivery
recipes/context ignores also select their required checks. Canonical transport
LICENSE changes rebuild app; provenance edits select transport alone.

Red-first contracts failed (exit 1) for the old container layout, missing image
notice, old tier selection and missing workflow/install/gate wiring. Final
contracts pass: 66 tier assertions, nine workflow tests (including actual image
conditions with missing evidence), and 27 transport source/package/delivery
contracts. ShellCheck for every tracked shell script, actionlint for all workflows,
YAML parsing/assertions for CI, timing and compose, Semgrep on changed tracked
files, gitleaks and diff-check exit 0. Final receipts and logs are attached on #578.
The dry reasoning artifact covers every CI job and the timing job; timing only
builds tests and executes its two ignored filters, so it needs no Python runtime.

Both complete images built (exit 0) from a clean Git archive at `8ed446b9`, without
external transport source access or dependency lifecycle scripts. Containers were
created only for file inspection/export and never started. The QA source and lock
match tracked bytes, its installed package link targets `../../../transport/js`,
and its working directory is `/app/web`. Both image notices hash to
`964cd7fcc4be51375162c2bbcf1b814e264d6477a24469530e73e1b113e616f2`.
Exported inventories contain no checked `.env`, `.git`, home-checkout or `.build`
paths. App static version output exists. No image was published.

An initially clean unrelated checkout at `644c0bf0` with an empty npm cache passed
transport `npm ci --prefix transport/js --ignore-scripts`, SPA `npm ci
--legacy-peer-deps`, web `npm ci`, web's 23 unit tests, SPA lint and Vite build.
Runtime sources/configs/tests and dependency manifests/locks remain byte-identical
through the final delivery commits; the equality receipt ties these runs to them.
The full SPA test/capture outcomes are recorded in the task's attached receipts.
Python's five baseline cases, four vector cases and all eight JS cases pass.
After frozen Python installation, compatibility tests ran with `UV_OFFLINE=true`,
including targeted Rust transport units, three Rust vector tests and three
Rust/Python harness tests (all exit 0). This is JS/Python and Rust/Python evidence,
not a direct JS/Rust runtime exchange.

The seven-day consumer release-age default and script suppression remain intact.
AGENTS.md and both consumer READMEs now document a temporary exact-package
`--min-release-age-exclude=package-name` exception for a deliberately reviewed
urgent release, retaining script suppression and the cooldown for its other
dependencies. No wildcard or persistent policy exception was added.

No dependency upgrades, crypto changes, gate weakening, main merge/push, CI
workflow dispatch, publication, paired/live QA, bridge binary or installer
execution, user-browser interaction, home-state access or network/firewall changes
are part of these receipts. Main CI after ordered #577/#578 integration remains
the live verification; broader ownership/reference work and final independent
review/integration remain #579/#580/#569.
