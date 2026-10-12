"""Package rejection cases use small archives, never crypto execution."""
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('packages', ROOT / 'scripts/transport_packages.py')
packages = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packages)


class PackageArchives(unittest.TestCase):
    def npm(self):
        return {f'package/{name}': (ROOT / 'transport/js' / name).read_bytes()
                for name in ('package.json', 'README.md', 'LICENSE.md', 'src/index.js')}

    def wheel(self):
        files = {f'build_secure_transport/{p.name}': p.read_bytes()
                 for p in (ROOT / 'transport/python/src/build_secure_transport').glob('*.py')}
        notice = (ROOT / 'transport/LICENSE.md').read_bytes()
        metadata = b'Name: build-secure-transport\nVersion: 0.1.0\nLicense-File: LICENSE.md\nLicense: '
        metadata += notice.rstrip().replace(b'\n', b'\n ') + b'\n\n'
        files['build_secure_transport-0.1.0.dist-info/METADATA'] = metadata
        files['build_secure_transport-0.1.0.dist-info/licenses/LICENSE.md'] = notice
        return files

    def test_valid_standalone_packages(self):
        packages.check_npm(self.npm(), ROOT)
        packages.check_python(self.wheel(), ROOT, wheel=True)

    def test_missing_npm_license_rejected(self):
        files = self.npm()
        del files['package/LICENSE.md']
        with self.assertRaisesRegex(ValueError, 'allowlist mismatch'):
            packages.check_npm(files, ROOT)

    def test_changed_source_rejected(self):
        for kind in ('npm', 'wheel'):
            with self.subTest(kind=kind):
                files = self.npm() if kind == 'npm' else self.wheel()
                path = 'package/src/index.js' if kind == 'npm' else 'build_secure_transport/device.py'
                files[path] += b'changed\n'
                with self.assertRaisesRegex(ValueError, 'bytes mismatch'):
                    if kind == 'npm':
                        packages.check_npm(files, ROOT)
                    else:
                        packages.check_python(files, ROOT, wheel=True)

    def test_generated_duplicate_source_rejected(self):
        files = self.wheel()
        files['build/lib/build_secure_transport/device.py'] = files['build_secure_transport/device.py']
        with self.assertRaisesRegex(ValueError, 'unexpected archive files'):
            packages.check_python(files, ROOT, wheel=True)

    def test_missing_python_source_rejected(self):
        files = self.wheel()
        del files['build_secure_transport/device.py']
        with self.assertRaisesRegex(ValueError, 'missing archive file'):
            packages.check_python(files, ROOT, wheel=True)

    def test_python_misleading_license_metadata_rejected(self):
        files = self.wheel()
        key = 'build_secure_transport-0.1.0.dist-info/METADATA'
        files[key] = b'Name: build-secure-transport\nVersion: 0.1.0\nLicense-File: LICENSE.md\nLicense: MIT\n\n'
        with self.assertRaisesRegex(ValueError, 'license metadata mismatch'):
            packages.check_python(files, ROOT, wheel=True)

    def test_archive_duplicate_path_and_symlink_rejected(self):
        for case in ('duplicate', 'symlink', 'traversal'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / 'invalid.tgz'
                with tarfile.open(path, 'w:gz') as archive:
                    entry = tarfile.TarInfo('package/LICENSE.md')
                    if case == 'symlink':
                        entry.type = tarfile.SYMTYPE
                        entry.linkname = '../../LICENSE'
                    if case == 'traversal':
                        entry.name = '../LICENSE.md'
                    archive.addfile(entry, io.BytesIO(b''))
                    if case == 'duplicate':
                        archive.addfile(entry, io.BytesIO(b''))
                with self.assertRaises(ValueError):
                    packages.archive_files(path)


if __name__ == '__main__':
    unittest.main()
