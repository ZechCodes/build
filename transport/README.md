# Build secure transport

The JS binding and Python reference live here, imported from
`ZechCodes/build-secure-transport` commit
`4bce3aacebd66a9674aaa96c6ec0d97ab4bcd793`. Package names and versions remain
`@build/secure-transport` / `build-secure-transport` 0.1.0.
[UPSTREAM.md](UPSTREAM.md) records the explicit file allowlist, hashes and
amendments; the initial import commit preserves every selected file byte.

The Rust implementation remains in `bridge/src/transport.rs`. Build's API owns
presence and device keys; the relay handles signaling and application traffic
uses WebRTC data channels. [ARCHITECTURE.md](../ARCHITECTURE.md#system-topology)
describes the current system. [E2E-MVP.md](E2E-MVP.md) is a historical protocol
document, with an annotation distinguishing its older topology.

This import is the first step of the
[consolidation plan](../planning/v2/Secure%20Transport%20Consolidation%20Plan.md).
Consumers still use the existing sibling dependency until the later cutover.
The imported JS/Python tests retain their baseline bytes; the portable interop
launcher and mandatory runtime compatibility evidence belong to #576.

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
