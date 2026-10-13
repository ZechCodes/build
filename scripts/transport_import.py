"""Import only pinned Git blobs, and check the recorded import without a sibling."""
import argparse
import hashlib
from pathlib import Path
import re
import subprocess
import sys

SOURCE_COMMIT = '4bce3aacebd66a9674aaa96c6ec0d97ab4bcd793'
# Explicit canonical paths, never directory globs or working-tree copies.
IMPORT_PATHS = (
    'E2E-MVP.md', 'LICENSE.md', 'README.md',
    'js/README.md', 'js/package-lock.json', 'js/package.json',
    'js/src/index.js', 'js/test/index.test.js',
    'python/README.md', 'python/pyproject.toml',
    'python/src/build_secure_transport/__init__.py',
    'python/src/build_secure_transport/_protocol.py',
    'python/src/build_secure_transport/device.py',
    'python/src/build_secure_transport/exceptions.py',
    'python/tests/interop_helper.py', 'python/tests/test_device.py', 'python/uv.lock',
)
# Crypto/runtime and license remain immutable; explicit harness amendments only.
AMENDABLE = {'README.md', 'E2E-MVP.md', 'js/package.json',
             'js/package-lock.json', 'python/pyproject.toml', 'js/test/index.test.js',
             'python/tests/test_device.py'}
LOCAL_FILES = {'.gitignore', 'UPSTREAM.md', 'js/LICENSE.md',
               'python/LICENSE.md', 'python/MANIFEST.in',
               'js/testing/python.mjs', 'js/test/vectors.test.js',
               'compatibility/test_vectors.py', 'testing/generate_vectors.py',
               'testing/generate_vectors.mjs', 'fixtures/v1.json',
               'fixtures/SHA256SUMS', 'fixtures/README.md'}
ROOT = Path(__file__).resolve().parents[1]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def git(upstream, *args):
    return subprocess.check_output(['git', '-C', str(upstream), *args])


def pinned_blobs(upstream):
    tree = {}
    for entry in git(upstream, 'ls-tree', '-r', '-z', SOURCE_COMMIT).split(b'\0'):
        if entry:
            metadata, path = entry.decode().split('\t')
            tree[path] = metadata.split()[0]
    for path in IMPORT_PATHS:
        if tree.get(path) != '100644':
            raise ValueError(f'not a regular tracked blob at pin: {path}')
    excluded = set(tree) - set(IMPORT_PATHS)
    if any(not (p == '.gitignore' or p.startswith('python/build/lib/')
                or p.startswith('python/src/build_secure_transport.egg-info/'))
           for p in excluded):
        raise ValueError(f'unclassified pinned files: {sorted(excluded)}')
    blobs = {p: git(upstream, 'show', f'{SOURCE_COMMIT}:{p}') for p in IMPORT_PATHS}
    return blobs, sorted(excluded)


def import_sources(upstream, root):
    blobs, excluded = pinned_blobs(upstream)
    destination = root / 'transport'
    if destination.is_symlink() or any((destination / p).exists() for p in IMPORT_PATHS):
        raise ValueError('import destination already contains source; refusing overwrite')
    for path, data in blobs.items():
        target = destination / path
        for parent in (target, *target.parents):
            if parent.is_symlink():
                raise ValueError(f'symlink in destination: {parent}')
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    rows = '\n'.join(f'| `{p}` | `{digest(data)}` | `{digest(data)}` |'
                     for p, data in blobs.items())
    exclusions = '\n'.join(f'- `{p}`' for p in excluded)
    (destination / 'UPSTREAM.md').write_text(f'''# Secure transport provenance

Source: `ZechCodes/build-secure-transport` at `{SOURCE_COMMIT}`.
Import only regular tracked blobs through `scripts/transport_import.py import`.
Working-tree bytes, symlinks and directories are never copied from the sibling.

## Explicit import allowlist and SHA-256

Each upstream path maps to `transport/<path>`. Source SHA is the unmodified
pinned blob; current SHA records separately reviewed metadata/doc amendments.
Executable sources, baseline tests and LICENSE.md must keep the source digest.

| Upstream path | Source SHA-256 | Current SHA-256 |
| --- | --- | --- |
{rows}

## Exclusions

Tracked files omitted at the pinned commit:

{exclusions}

Also excluded regardless of checkout state: `.git/`, `.rift/`, `node_modules/`,
`.venv/`, caches, build artifacts and the untracked recursive
`build-secure-transport` symlink. Build's existing root ignores are retained.

## Amendments

None in the initial import. The initial import commit preserves all listed bytes.
''')
    print(f'imported {len(blobs)} pinned blobs; excluded {len(excluded)} tracked files')


def manifest(root):
    text = (root / 'transport/UPSTREAM.md').read_text()
    if f'`{SOURCE_COMMIT}`' not in text:
        raise ValueError('provenance source commit mismatch')
    rows = re.findall(r'^\| `([^`]+)` \| `([0-9a-f]{64})` \| `([0-9a-f]{64})` \|$',
                      text, re.MULTILINE)
    if len(rows) != len(IMPORT_PATHS) or {r[0] for r in rows} != set(IMPORT_PATHS):
        raise ValueError('provenance allowlist mismatch')
    for path, source, current in rows:
        if path not in AMENDABLE and source != current:
            raise ValueError(f'immutable source amendment: {path}')
    return rows


def regular_bytes(path):
    if any(p.is_symlink() for p in (path, *path.parents)):
        raise ValueError(f'symlink: {path}')
    return path.read_bytes()


def source_files(root):
    # Installed ignored environments are not source. In archives, inspect every
    # member; in Git checkouts, include tracked and unignored new files.
    if (root / '.git').exists():
        paths = git(root, 'ls-files', '-z', '--cached', '--others', '--exclude-standard',
                    '--', 'transport').split(b'\0')
        return [root / p.decode() for p in paths if p]
    return (root / 'transport').rglob('*')


def check_import(root, upstream=None):
    rows = manifest(root)
    for path, _, current in rows:
        if digest(regular_bytes(root / 'transport' / path)) != current:
            raise ValueError(f'digest mismatch: {path}')
    for path in source_files(root):
        relative = path.relative_to(root / 'transport').as_posix()
        if path.is_symlink():
            raise ValueError(f'symlink: {relative}')
        if path.is_file() and '__pycache__' not in path.parts:
            if relative not in IMPORT_PATHS and relative not in LOCAL_FILES and not relative.startswith('tests/'):
                raise ValueError(f'unexpected file: {relative}')
    if upstream:
        blobs, excluded = pinned_blobs(upstream)
        for path, source, _ in rows:
            if digest(blobs[path]) != source:
                raise ValueError(f'pinned source digest mismatch: {path}')
            if path not in AMENDABLE and regular_bytes(root / 'transport' / path) != blobs[path]:
                raise ValueError(f'pinned source bytes mismatch: {path}')
        print(f'compared {len(rows)} original digests and immutable bytes; {len(excluded)} exclusions')
    print(f'checked {len(rows)} imported file digests at {SOURCE_COMMIT}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['import', 'check'])
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--upstream', type=Path)
    args = parser.parse_args()
    if args.operation == 'import' and not args.upstream:
        parser.error('import requires --upstream (read-only Git source)')
    try:
        if args.operation == 'import':
            import_sources(args.upstream, args.root)
        else:
            check_import(args.root, args.upstream)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
