# Build secure transport

Build owns maintenance, audit and review of the JS binding and Python reference
in this directory and the Rust implementation at `bridge/src/transport.rs`.
The original import came from `ZechCodes/build-secure-transport` commit
`4bce3aacebd66a9674aaa96c6ec0d97ab4bcd793`. Package names and versions remain
`@build/secure-transport` / `build-secure-transport` 0.1.0.
[UPSTREAM.md](UPSTREAM.md) records the allowlist, hashes and later amendments;
the initial import `496716692caa3a711a7f627952d711adfa52ac73` preserves every
selected byte. The completed consumer/CI/image cutover landed on Build main at
`eaf01df8d2ed3e0ef252edbcc38124d14bf2de0f`. The Build commit now pins shipped
transport source; the old pin is provenance, not a container build argument.

The legacy repository stays available and untouched for history and rollback;
it is no longer the maintenance source of truth. The plan does not archive it
or assert that GitHub has made it read-only. A legacy README pointer is drafted
on #579 for Zech to apply manually, as a separate action. New fixes and reviews
belong in Build. Report vulnerabilities privately through
[Build’s security policy](../SECURITY.md), including transport findings.

Build’s API owns presence and device keys. The relay handles session setup and
signaling; application traffic uses WebRTC DataChannels, directly or through
TURN, with no relay data-plane fallback.
[ARCHITECTURE.md](../ARCHITECTURE.md#system-topology) describes the current
system. [E2E-MVP.md](E2E-MVP.md) preserves the historical envelope document,
with an annotation distinguishing its older topology. The bindings do not
provide replay filtering or authenticate outer `route_to` routing metadata.
The move changes no protocol, API, session, identity or network behavior.

The imported [JS README](js/README.md) and [Python README](python/README.md)
are pinned baseline artifacts: imported, do not edit here. Ownership, setup
and current compatibility instructions live in this README so their original
bytes remain auditable. Future source imports require a separate reviewed
change with an explicit pin, allowlist, hashes and compatibility evidence;
see [the import policy](UPSTREAM.md#maintenance-and-future-imports).

## Development and compatibility

SPA and web resolve `file:../transport/js` with committed portable consumer
locks. Use Node 22.23.3/npm 12.0.2 and install the binding first from Build’s root:

```sh
nice -n 10 npm ci --prefix transport/js --ignore-scripts
nice -n 10 npm ci --prefix spa --legacy-peer-deps
nice -n 10 npm ci --prefix web
```

Consumer `.npmrc` files suppress lifecycle scripts and require seven days before
new registry resolutions; SPA also records legacy peer resolution. Dependency
edits use SPA `npm install --legacy-peer-deps` or web `npm install` and commit
the lock. The [SPA](../spa/README.md#prereqs) and
[web](../web/README.md#the-suites) READMEs document the reviewed one-command
exact-package release-age exception. Node resolves sodium from transport’s own
installed dependencies; Vite’s existing shim uses SPA-local sodium. Both locks
retain libsodium/libsodium-wrappers 0.7.16.

Python is a reference/test dependency, not a production app or bridge runtime
dependency. CI pins uv 0.12.13 and uses frozen resolution. From Build’s root:

```sh
nice -n 10 uv sync --frozen --project transport/python
nice -n 10 uv run --frozen --project transport/python python -m unittest discover -s transport/python/tests -v
nice -n 10 uv run --frozen --project transport/python python -m unittest discover -s transport/compatibility -v
nice -n 10 npm test --prefix transport/js
nice -n 10 cargo test --locked --manifest-path bridge/Cargo.toml --lib transport
nice -n 10 cargo test --locked --manifest-path bridge/Cargo.toml --test transport_vectors --test interop_python
```

The original five Python and four JS cases remain; four added Python vector cases
and four added JS cases are separately counted. The Rust/Python and JS/Python
launchers default to this checkout and fail on missing uv or reference files.
`BUILD_SECURE_TRANSPORT_PY` and `BST_UV` are deliberate test overrides. These
checks establish JS/Python and Rust/Python compatibility, not a direct JS/Rust
runtime exchange. [Fixtures](fixtures/README.md) preserve old/new evidence.
The dedicated transport CI job runs the contracts, units, vectors and exchanges;
owed transport checks must succeed before either image builds.

## License scope

[LICENSE.md](LICENSE.md) preserves the upstream Business Source License 1.1
notice, including Zechariah Zimmerman as licensor, the four-year change period
and MIT change license. The imported work retains that notice as an exception
to Build's root AGPL-3.0-only scope. The move does not establish that a change
date has elapsed or grant new rights. The historical upstream README's claim
that Build's client/platform were proprietary has been replaced here with the
current scoped notices; the original README digest remains in UPSTREAM.md.

`js/LICENSE.md` and `python/LICENSE.md` are exact copies for standalone package
artifacts, never symlinks to files outside a package. npm metadata says
`SEE LICENSE IN LICENSE.md`; its explicit `files` list packages only source,
README and notice. Python retains setuptools >=69 and uses its supported
file-based license metadata and `license-files` setting. Its sdist contains
canonical `src/` and baseline tests; its wheel contains only the canonical
Python modules and generated distribution metadata, including the notice.
Generated sdist egg-info is packaging metadata, not an imported source tree.

## Import verification

From Build's root, with Python 3.11+ (stdlib `tomllib` for package tests):

```sh
nice -n 10 python -m unittest discover -s transport/tests -v
nice -n 10 python scripts/transport_import.py check
```

The check needs no external repository. For independent comparison, add
`--upstream /path/to/read-only/build-secure-transport`; it reads Git blobs at
exactly the recorded commit, ignoring the sibling's working tree. The `import`
operation is only for reproducing the initial import into a fresh destination
(`--root /path/to/scratch --upstream ...`); it refuses existing source files.
Run import checks on a clean source tree, before installing dependencies or
creating local package outputs.

## Local package checks

Coordinate a small packaging slot with the merge queue, run serially at nice 10,
and record command exits and versions. These commands create local artifacts;
there is no publishing step. Use a committed tree in a disposable directory to
keep generated outputs out of the checkout:

```sh
package_dir=$(mktemp -d)
git archive HEAD transport | tar -x -C "$package_dir" --strip-components=1
mkdir "$package_dir/artifacts"
nice -n 10 npm pack "$package_dir/js" --ignore-scripts --json \
  --pack-destination "$package_dir/artifacts"
UV_CONCURRENT_BUILDS=1 nice -n 10 uv build "$package_dir/python" \
  --out-dir "$package_dir/artifacts"
nice -n 10 python scripts/transport_packages.py \
  --npm "$package_dir/artifacts/build-secure-transport-0.1.0.tgz" \
  --sdist "$package_dir/artifacts/build_secure_transport-0.1.0.tar.gz" \
  --wheel "$package_dir/artifacts/build_secure_transport-0.1.0-py3-none-any.whl"
```

The checker reads archive members without extraction, verifies source/license
bytes and metadata, prints file listings, and rejects unexpected source copies,
missing files, duplicate paths and symlink members. Keep artifact hashes and
receipts with the [evidence ledger](../planning/v2/Secure%20Transport%20Consolidation%20Evidence.md).
Runtime dependencies remain pinned by the imported JS/Python locks. Packaging
build tools resolve according to the retained setuptools requirement; record the
actual backend version in packaging receipts. No dependency install, crypto
change or claim of runtime compatibility is implied by a package-content check.
