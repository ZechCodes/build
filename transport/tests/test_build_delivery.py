"""Single-checkout image contracts, without starting the application stack."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class BuildDelivery(unittest.TestCase):
    def test_images_use_local_locked_transport(self):
        for path in ('skriftapp/Containerfile', 'web/Containerfile'):
            with self.subTest(path=path):
                recipe = (ROOT / path).read_text()
                self.assertNotIn('git clone', recipe)
                self.assertNotIn('BUILD_SECURE_TRANSPORT_COMMIT', recipe)
                self.assertNotIn('npm pkg set', recipe)
                self.assertIn('COPY transport/js/package.json transport/js/package-lock.json', recipe)
                self.assertIn('COPY transport/js/src/', recipe)
                self.assertIn('npm ci --ignore-scripts', recipe)
                self.assertIn('npm@12.0.2', recipe)
                self.assertIn('node:22.23.4-bookworm-slim', recipe)

    def test_consumer_locks_and_policy_reach_images(self):
        app = (ROOT / 'skriftapp/Containerfile').read_text()
        web = (ROOT / 'web/Containerfile').read_text()
        self.assertIn('COPY spa/package.json spa/package-lock.json spa/.npmrc', app)
        self.assertIn('npm ci --legacy-peer-deps', app)
        self.assertIn('COPY web/package.json web/package-lock.json web/.npmrc', web)
        self.assertIn('npm ci --omit=dev --ignore-scripts', web)
        self.assertIn('WORKDIR /app/web', web)
        self.assertNotIn('COPY . ', web)
        self.assertIn('COPY transport/LICENSE.md ./licenses/secure-transport-LICENSE.md', app)

    def test_root_context_and_exclusions(self):
        compose = (ROOT / 'deploy/compose.real.yml').read_text().split('\n  qa:\n', 1)[1]
        self.assertIn('context: ..\n', compose)
        self.assertIn('dockerfile: web/Containerfile', compose)
        for name in ('.dockerignore', '.containerignore'):
            rules = (ROOT / name).read_text().splitlines()
            self.assertNotIn('spa/package-lock.json', rules)
            for rule in ('transport/js/node_modules', 'transport/python/.venv',
                         'transport/python/build', '**/.env', '**/.env.*'):
                self.assertIn(rule, rules, (name, rule))


if __name__ == '__main__':
    unittest.main()
