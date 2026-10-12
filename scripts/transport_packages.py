"""Check local npm, Python sdist and wheel bytes without extracting archives."""
import argparse
from email.parser import BytesParser
import json
from pathlib import Path, PurePosixPath
import stat
import sys
import tarfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
PYTHON_SOURCES = ('__init__.py', '_protocol.py', 'device.py', 'exceptions.py')


def archive_files(path):
    files = {}
    if path.suffix == '.whl':
        with zipfile.ZipFile(path) as archive:
            for entry in archive.infolist():
                if not entry.is_dir():
                    if stat.S_ISLNK(entry.external_attr >> 16):
                        raise ValueError(f'archive symlink: {entry.filename}')
                    add_file(files, entry.filename, archive.read(entry))
    else:
        with tarfile.open(path, 'r:gz') as archive:
            for entry in archive.getmembers():
                if entry.isdir():
                    continue
                if not entry.isfile():
                    raise ValueError(f'non-regular archive member: {entry.name}')
                add_file(files, entry.name, archive.extractfile(entry).read())
    return files


def add_file(files, name, data):
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts or name in files:
        raise ValueError(f'unsafe or duplicate archive path: {name}')
    files[name] = data


def same_bytes(files, name, expected):
    if name not in files:
        raise ValueError(f'missing archive file: {name}')
    if files[name] != expected:
        raise ValueError(f'archive bytes mismatch: {name}')


def check_npm(files, root):
    expected = {'package/package.json', 'package/README.md',
                'package/LICENSE.md', 'package/src/index.js'}
    if set(files) != expected:
        raise ValueError(f'npm file allowlist mismatch: {sorted(set(files) ^ expected)}')
    for name in expected:
        local = root / 'transport/js' / name.removeprefix('package/')
        same_bytes(files, name, local.read_bytes())
    same_bytes(files, 'package/LICENSE.md', (root / 'transport/LICENSE.md').read_bytes())
    metadata = json.loads(files['package/package.json'])
    if (metadata.get('name'), metadata.get('version'), metadata.get('license')) != (
        '@build/secure-transport', '0.1.0', 'SEE LICENSE IN LICENSE.md'
    ):
        raise ValueError('npm license/name/version metadata mismatch')


def normalized_notice(text):
    return '\n'.join(line.strip() for line in text.strip().splitlines())


def check_python_metadata(data, notice):
    metadata = BytesParser().parsebytes(data)
    if (metadata['Name'], metadata['Version']) != ('build-secure-transport', '0.1.0'):
        raise ValueError('Python name/version metadata mismatch')
    if metadata.get_all('License-File') != ['LICENSE.md']:
        raise ValueError('Python License-File metadata mismatch')
    if normalized_notice(metadata.get('License', '')) != normalized_notice(notice.decode()):
        raise ValueError('Python license metadata mismatch')


def python_source_map(root, prefix):
    return {f'{prefix}{name}': (root / 'transport/python/src/build_secure_transport' / name).read_bytes()
            for name in PYTHON_SOURCES}


def check_python(files, root, wheel):
    notice = (root / 'transport/LICENSE.md').read_bytes()
    if wheel:
        source_map = python_source_map(root, 'build_secure_transport/')
        licenses = [p for p in files if p.endswith('.dist-info/licenses/LICENSE.md')
                    or p.endswith('.dist-info/LICENSE.md')]
        metadata_paths = [p for p in files if p.endswith('.dist-info/METADATA')]
        if len(licenses) != 1 or len(metadata_paths) != 1:
            raise ValueError('wheel must contain one license and one METADATA')
        allowed = set(source_map) | {licenses[0], metadata_paths[0]}
        allowed |= {str(PurePosixPath(metadata_paths[0]).parent / p)
                    for p in ('WHEEL', 'RECORD', 'top_level.txt')}
    else:
        prefixes = {PurePosixPath(p).parts[0] for p in files}
        if len(prefixes) != 1:
            raise ValueError('sdist must contain a single root')
        prefix = next(iter(prefixes)) + '/'
        source_map = python_source_map(root, prefix + 'src/build_secure_transport/')
        licenses = [prefix + 'LICENSE.md']
        metadata_paths = [prefix + 'PKG-INFO', prefix + 'src/build_secure_transport.egg-info/PKG-INFO']
        allowed = set(source_map) | set(licenses) | set(metadata_paths)
        allowed |= {prefix + p for p in ('README.md', 'pyproject.toml', 'MANIFEST.in', 'uv.lock', 'setup.cfg')}
        allowed |= {prefix + 'src/build_secure_transport.egg-info/' + p for p in
                    ('SOURCES.txt', 'dependency_links.txt', 'requires.txt', 'top_level.txt')}
        for name in ('interop_helper.py', 'test_device.py'):
            source_map[prefix + 'tests/' + name] = (root / 'transport/python/tests' / name).read_bytes()
        allowed |= set(source_map)
        for name in ('README.md', 'pyproject.toml', 'MANIFEST.in', 'uv.lock'):
            same_bytes(files, prefix + name, (root / 'transport/python' / name).read_bytes())
    if set(files) - allowed:
        raise ValueError(f'unexpected archive files: {sorted(set(files) - allowed)}')
    for name, expected in source_map.items():
        same_bytes(files, name, expected)
    for name in licenses:
        same_bytes(files, name, notice)
    for name in metadata_paths:
        if name not in files:
            raise ValueError(f'missing archive file: {name}')
        check_python_metadata(files[name], notice)


def check_package(kind, path, root=ROOT):
    files = archive_files(path)
    if kind == 'npm':
        check_npm(files, root)
    else:
        check_python(files, root, kind == 'wheel')
    print(f'{kind}: {path.name}: checked {len(files)} files, canonical bytes and license metadata')
    for name in sorted(files):
        print(f'  {name}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    for kind in ('npm', 'sdist', 'wheel'):
        parser.add_argument('--' + kind, type=Path, required=True)
    args = parser.parse_args()
    try:
        for kind in ('npm', 'sdist', 'wheel'):
            check_package(kind, getattr(args, kind), args.root)
    except (ValueError, OSError, tarfile.TarError, zipfile.BadZipFile) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
