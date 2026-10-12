"""Import contracts; run from Build with unittest discover -s transport/tests."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
CHECK = ROOT / 'scripts/transport_import.py'


class ImportContract(unittest.TestCase):
    def check(self, root=ROOT, *extra):
        return subprocess.run(
            ['python', str(CHECK), 'check', '--root', str(root), *extra],
            text=True, capture_output=True, check=False,
        )

    def test_imported_tree_matches_pinned_manifest(self):
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def mutate(self, relative, data):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copytree(ROOT / 'transport', root / 'transport',
                            ignore=shutil.ignore_patterns('__pycache__', '.venv', 'node_modules', '*.egg-info'))
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            return self.check(root)

    def test_source_drift_fails(self):
        result = self.mutate('transport/js/src/index.js', b'changed crypto\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('digest mismatch', result.stderr)

    def test_license_drift_fails(self):
        result = self.mutate('transport/LICENSE.md', b'AGPL\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('digest mismatch', result.stderr)

    def test_generated_source_cannot_enter_allowlist(self):
        result = self.mutate('transport/python/build/lib/build_secure_transport/device.py', b'duplicate\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('unexpected file', result.stderr)

    def test_symlink_cannot_stand_in_for_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copytree(ROOT / 'transport', root / 'transport',
                            ignore=shutil.ignore_patterns('__pycache__', '.venv', 'node_modules', '*.egg-info'))
            path = root / 'transport/js/src/index.js'
            data = path.read_bytes()
            path.unlink()
            other = root / 'source.js'
            other.write_bytes(data)
            path.symlink_to(other)
            result = self.check(root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('symlink', result.stderr)


if __name__ == '__main__':
    unittest.main()
