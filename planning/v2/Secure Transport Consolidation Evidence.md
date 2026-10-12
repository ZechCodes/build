# Secure transport inventory and evidence

Companion to [the #572 migration plan](Secure%20Transport%20Consolidation%20Plan.md).
Inspection date: 2026-10-12. The original #572 planning inventory is retained;
#575 local receipts below cover only import/package evidence P1/P2.
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
| C1 | Python 5-case and JS 4-test baselines retained/passing; later added cases separately counted | #576 | pending |
| C2 | Rust transport tests and actual Rust/Python + JS/Python exchanges pass; missing prerequisite fails | #576 | pending |
| C3 | Non-secret old/new vectors cover identity/bootstrap/accept/data/close and malformed inputs, Unicode/nested payloads; no nonce reuse introduced | #576 | pending |
| D1 | Independent clean checkout paths install from locks and resolve in-tree source without sibling symlinks or old dependencies | #577 | pending |
| D2 | Node and Vite libsodium resolution; SPA lint/focused tests/build; web unit tests | #577 | pending |
| B1 | App and QA images build from clean repository context without external transport access; final contents/notices reviewed | #578 | pending |
| B2 | Root Docker/Podman ignores and narrow COPY rules exclude environments/private files and retain necessary locks | #578 | pending |
| G1 | JS-only, Python-only, Rust, fixture, workflow/selector and unrelated-doc path cases select intended checks/images | #578 | pending |
| G2 | Failed/skipped/canceled/missing compatibility evidence blocks owed image builds, including a later app-only commit | #578 | pending |
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

Only P1/P2 are verified here. C1–R2 remain pending; this is neither runtime
compatibility evidence nor the final migration security checklist/integration.
Independent exact-head review is still required.

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
