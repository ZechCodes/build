"""Consumer installs must be reproducible within one checkout."""
import json
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


def read_json(path):
    return json.loads((ROOT / path).read_text())


class ConsumerDependencies(unittest.TestCase):
    def test_consumers_use_the_in_tree_binding(self):
        for consumer in ('spa', 'web'):
            with self.subTest(consumer=consumer):
                package = read_json(f'{consumer}/package.json')
                self.assertEqual(package['dependencies']['@build/secure-transport'],
                                 'file:../transport/js')

    def test_consumer_locks_are_portable_and_match_manifests(self):
        for consumer in ('spa', 'web'):
            with self.subTest(consumer=consumer):
                package = read_json(f'{consumer}/package.json')
                lock = read_json(f'{consumer}/package-lock.json')
                self.assertEqual(lock['lockfileVersion'], 3)
                for field in ('dependencies', 'devDependencies'):
                    self.assertEqual(lock['packages'][''][field], package[field])
                binding = lock['packages']['node_modules/@build/secure-transport']
                self.assertEqual(binding, {'resolved': '../transport/js', 'link': True})
                for path, entry in lock['packages'].items():
                    self.assertFalse(path.startswith(('/', '../../')))
                    if 'resolved' not in entry:
                        continue
                    resolved = entry['resolved']
                    self.assertTrue(resolved.startswith('https://registry.npmjs.org/')
                                    or resolved == '../transport/js', resolved)

    def test_locks_are_not_gitignored(self):
        result = subprocess.run(
            ['git', 'check-ignore', 'spa/package-lock.json', 'web/package-lock.json'],
            cwd=ROOT, text=True, capture_output=True, check=False,
        )
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)

    def test_install_policy_pins_tools_and_disables_scripts(self):
        for consumer in ('spa', 'web'):
            with self.subTest(consumer=consumer):
                package = read_json(f'{consumer}/package.json')
                self.assertEqual(package['packageManager'], 'npm@12.0.2')
                self.assertEqual(package['engines']['node'], '>=22.23.4 <23')
                self.assertEqual((ROOT / consumer / '.nvmrc').read_text().strip(), '22.23.4')
                config = (ROOT / consumer / '.npmrc').read_text().splitlines()
                self.assertIn('ignore-scripts=true', config)
                self.assertIn('min-release-age=7', config)
        self.assertIn('legacy-peer-deps=true', (ROOT / 'spa/.npmrc').read_text().splitlines())

    def test_sodium_resolutions_retain_the_transport_baseline(self):
        baseline = read_json('transport/js/package-lock.json')['packages']
        packages = read_json('spa/package-lock.json')['packages']
        for dependency in ('libsodium', 'libsodium-wrappers'):
            path = f'node_modules/{dependency}'
            for field in ('version', 'resolved', 'integrity'):
                self.assertEqual(packages[path][field], baseline[path][field])


if __name__ == '__main__':
    unittest.main()
