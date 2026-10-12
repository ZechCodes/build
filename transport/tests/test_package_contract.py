"""Licensing must survive copying each package out of the monorepo."""
import json
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


class PackageContract(unittest.TestCase):
    def test_npm_scoped_license_and_explicit_contents(self):
        package = json.loads((ROOT / 'transport/js/package.json').read_text())
        self.assertEqual(package.get('license'), 'SEE LICENSE IN LICENSE.md')
        self.assertEqual(set(package.get('files', [])), {'src', 'README.md', 'LICENSE.md'})
        lock = json.loads((ROOT / 'transport/js/package-lock.json').read_text())
        self.assertEqual(lock['packages'][''].get('license'), package['license'])

    def test_python_scoped_license_and_inclusion(self):
        package = tomllib.loads((ROOT / 'transport/python/pyproject.toml').read_text())
        self.assertEqual(package['project'].get('license'), {'file': 'LICENSE.md'})
        self.assertEqual(package['tool']['setuptools'].get('license-files'), ['LICENSE.md'])

    def test_standalone_package_licenses_match_preserved_notice(self):
        notice = (ROOT / 'transport/LICENSE.md').read_bytes()
        for package in ('js', 'python'):
            with self.subTest(package=package):
                self.assertEqual((ROOT / f'transport/{package}/LICENSE.md').read_bytes(), notice)


if __name__ == '__main__':
    unittest.main()
