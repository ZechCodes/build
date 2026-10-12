# Secure transport provenance

Source: `ZechCodes/build-secure-transport` at `4bce3aacebd66a9674aaa96c6ec0d97ab4bcd793`.
This is the historical import source, not the active maintenance repository.
Build owns the in-tree JS/Python work and the existing Rust implementation.
The original pinned clone in the app recipe was removed; app/QA images and CI
now use the Build checkout, with the cutover landed at
`eaf01df8d2ed3e0ef252edbcc38124d14bf2de0f`.

Import only regular tracked blobs through `scripts/transport_import.py import`.
Working-tree bytes, symlinks and directories are never copied from the sibling.

## Explicit import allowlist and SHA-256

Each upstream path maps to `transport/<path>`. Source SHA is the unmodified
pinned blob; current SHA records separately reviewed metadata/doc amendments.
Crypto/runtime sources, LICENSE.md and the language READMEs must keep the
source digest at this migration baseline. Explicit harness and root README
amendments retain the original digest and record their current digest.
`js/README.md` and `python/README.md` are imported, do not edit here; current
ownership/setup is in [README.md](README.md).

| Upstream path | Source SHA-256 | Current SHA-256 |
| --- | --- | --- |
| `E2E-MVP.md` | `984ddc9c7582a6340b073274602d03f3e50a889d2ae8deb42de9f3b7d4e35c38` | `c606c628deff57f5b357292f157e9e5559a84b490838c0522e57e7b285a22574` |
| `LICENSE.md` | `964cd7fcc4be51375162c2bbcf1b814e264d6477a24469530e73e1b113e616f2` | `964cd7fcc4be51375162c2bbcf1b814e264d6477a24469530e73e1b113e616f2` |
| `README.md` | `4794e30bbf5b744b5b05f1186cdd804185cc6fc673f6ec3d8fd4804f39e6d54e` | `f88ca3c02b82891e9bf3941039b2fcc9b185706f723b690f1286af2efd220a32` |
| `js/README.md` | `5cc959b5d8fe9a31cb273a18eca62840debb441b4294680c0a878c955e0a75b1` | `5cc959b5d8fe9a31cb273a18eca62840debb441b4294680c0a878c955e0a75b1` |
| `js/package-lock.json` | `fe06ddb52117f0767fba5e613167a446a13879ff07bfd5417a4c317dd3ebba5f` | `709c8ce1b4e4b13507a87cb8df4c5dba046f15122a530adaed73eda1bb074b21` |
| `js/package.json` | `17ad3fb3cf0874c4c640295620113ad167741ce1d9a910f5d7519492c85a4208` | `5527f61d426144c6858879ae2fe0f35f96a94481766f722b3b1af44b5f2cbce4` |
| `js/src/index.js` | `55f85d864691aa4915a32876ff1511af2d24083adeba744d9935a834b64dc443` | `55f85d864691aa4915a32876ff1511af2d24083adeba744d9935a834b64dc443` |
| `js/test/index.test.js` | `5dfd2c9fbe0a6eec6edb625821a2c350513e0e940d6c5177d2bb6404b8954ef4` | `36f81390638dd83b5afd48d016332a8c7df382cfd6055b5a45169f683d004cbc` |
| `python/README.md` | `d7f6e12fd9feb8e7472c1cd013c91eedbe3a894401309eee407689ece0d9e8fd` | `d7f6e12fd9feb8e7472c1cd013c91eedbe3a894401309eee407689ece0d9e8fd` |
| `python/pyproject.toml` | `e7ad92d3d1bcf82ee7176b9d9a23568be303fdb5d1fa9e68ae26f70082e888a6` | `25066f60497ba6f2c889199c9cfa8d3c15931609e4cc7da981ccbc39b40e3202` |
| `python/src/build_secure_transport/__init__.py` | `0cc488cf4f82833d6fe7547773eed70f3f9b1281b73f155b70141ac7817c3a19` | `0cc488cf4f82833d6fe7547773eed70f3f9b1281b73f155b70141ac7817c3a19` |
| `python/src/build_secure_transport/_protocol.py` | `9e0e177ac186a41f65c2169ea328b94a397a46a1f0c1359ce2110666dc7015ed` | `9e0e177ac186a41f65c2169ea328b94a397a46a1f0c1359ce2110666dc7015ed` |
| `python/src/build_secure_transport/device.py` | `6bfef20825ee35e7294ed80d53b944c14869724f39d89905a8418c8beada635b` | `6bfef20825ee35e7294ed80d53b944c14869724f39d89905a8418c8beada635b` |
| `python/src/build_secure_transport/exceptions.py` | `2ca01dce41552dc196c65f76f488f7a8f748d9f8419fd26b156a31f154c63548` | `2ca01dce41552dc196c65f76f488f7a8f748d9f8419fd26b156a31f154c63548` |
| `python/tests/interop_helper.py` | `828b973fe961fd1d07fb77935407df8d75f51874d2e6440bbb4308501f668b8b` | `828b973fe961fd1d07fb77935407df8d75f51874d2e6440bbb4308501f668b8b` |
| `python/tests/test_device.py` | `27a275599f0888af302667848df76d5e6ee2b712ab357acf6c110740d66dd24c` | `27a275599f0888af302667848df76d5e6ee2b712ab357acf6c110740d66dd24c` |
| `python/uv.lock` | `800e16b959c20ba90bd4341f44bf1ef3671233c9edab2fee8584f21f83cec97d` | `800e16b959c20ba90bd4341f44bf1ef3671233c9edab2fee8584f21f83cec97d` |

## Exclusions

Tracked files omitted at the pinned commit:

- `.gitignore`
- `python/build/lib/build_secure_transport/__init__.py`
- `python/build/lib/build_secure_transport/_protocol.py`
- `python/build/lib/build_secure_transport/device.py`
- `python/build/lib/build_secure_transport/exceptions.py`
- `python/src/build_secure_transport.egg-info/PKG-INFO`
- `python/src/build_secure_transport.egg-info/SOURCES.txt`
- `python/src/build_secure_transport.egg-info/dependency_links.txt`
- `python/src/build_secure_transport.egg-info/requires.txt`
- `python/src/build_secure_transport.egg-info/top_level.txt`

Also excluded regardless of checkout state: `.git/`, `.rift/`, `node_modules/`,
`.venv/`, caches, build artifacts and the untracked recursive
`build-secure-transport` symlink. Build's existing root ignores are retained.

## Amendments

The initial import commit `496716692caa3a711a7f627952d711adfa52ac73`
preserves all listed source bytes. Subsequent #575 amendments are separate:

- `README.md`: replace obsolete proprietary-product language with current
  topology, scoped licensing, import and local packaging instructions.
- `E2E-MVP.md`: prepend a historical-topology annotation; original body intact.
- `js/package.json`: add file-based license metadata and an explicit package
  contents allowlist. Name, version, exports, scripts and dependencies unchanged.
- `js/package-lock.json`: add only the root package license field. Resolved
  libsodium and libsodium-wrappers remain 0.7.16; integrity entries unchanged.
- `python/pyproject.toml`: add file-based license metadata, explicit license
  inclusion and disable implicit package data. Build/runtime requirements,
  package name/version and source discovery unchanged; `uv.lock` unchanged.

Added `js/LICENSE.md` and `python/LICENSE.md` are byte-identical copies of the
preserved `LICENSE.md`, for standalone artifacts. `python/MANIFEST.in` includes
the notice, lock and canonical tests, and prunes generated source/cache outputs.
The reviewed transport `.gitignore` adds artifact and environment exclusions;
it does not replace Build's root ignores. Local import/package checks and their
regression tests are Build-owned additions. Root README records the license
exception; it does not overwrite the AGPL root LICENSE or the existing Rust
transport's license scope.

## #576 harness and compatibility amendments

- `js/test/index.test.js`: retain the same four cases and assertions; replace
  the transport-root `.venv/bin/python` assumption with the explicit frozen
  `js/testing/python.mjs` launcher. The default Python reference is in-tree;
  `BUILD_SECURE_TRANSPORT_PY` and `BST_UV` are deliberate test overrides.
- Rust's `bridge/tests/interop_python.rs` is a Build-owned harness amendment,
  independent of this upstream import. `bridge/src/transport.rs` stays unchanged.
- Added `compatibility/test_vectors.py`, JS vector consumers/generators and immutable non-secret `fixtures/v1.json`
  are test material, documented and hashed in `fixtures/README.md`/`SHA256SUMS`.
  They are not runtime package exports. The five original Python cases and
  original `interop_helper.py` remain byte-identical.
- The import checker now permits only the named test additions/amendment. In a
  Git checkout it enumerates tracked plus unignored files, excluding installed
  ignored environments; an exported tree is still inspected in full.
  Crypto/runtime source and notice drift remain errors.

Dependency manifests/locks are unchanged by #576, including resolved sodium
0.7.16, PyNaCl 1.6.2, cffi 2.0.0 and pycparser 3.0.

## #579 documentation amendment

`README.md` now records the completed one-checkout cutover, Build ownership and
private reporting destination, frozen development/tests, preserved language
README bytes, and the legacy repository boundary. Its source digest remains the
original pinned blob; only its current digest changes. No language README,
crypto, fixture, dependency, manifest, lock or license bytes change in #579.

## Maintenance and future imports

After the verified cutover, Build is the maintenance/audit source of truth.
Develop fixes here through normal task, branch, test and review gates; changes
are pinned by the Build commit. The old repository stays reachable and untouched
for provenance and rollback. It is not archived by this task, and no read-only
hosting status is asserted. #579’s task comment drafts a README pointer for
Zech to apply by hand; applying it is separately coordinated.

There is no automatic upstream sync or live clone in the build. If a future
change is imported from the legacy repository or another source, review it as
an explicit import: name the source and immutable commit, select canonical
tracked regular blobs, record source/current hashes and exclusions, preserve
license notices, and rerun package-content and old/new compatibility checks.
Reconcile the checker’s recorded pin/allowlists only within that separately
reviewed change; the current `import` operation reproduces the original pin
into a fresh destination and refuses existing source files. Do not rerun it
over maintained source. Keep baseline provenance distinguishable from later
maintenance, rather than replacing original hashes with current bytes.
