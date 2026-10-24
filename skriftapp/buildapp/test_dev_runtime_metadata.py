"""Runtime imports must resolve framework/custom FKs in a fresh process."""
from pathlib import Path

import pytest
import yaml

from buildapp.dev_runtime_test_support import run_runtime_probe

ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.parametrize('config_name', ['app.channel-dev.yaml', 'app.yaml'])
def test_cold_runtime_metadata_resolves_every_foreign_key(tmp_path, monkeypatch, config_name):
    config = yaml.safe_load((ROOT / config_name).read_text())
    (tmp_path / 'app.yaml').write_text(yaml.safe_dump(config))
    (tmp_path / 'templates').symlink_to(ROOT / 'templates')
    for key, value in {
        'SKRIFT_ENV': 'production', 'SECRET_KEY': 'disposable-metadata-proof-secret',
        # create_app constructs an engine but the metadata probe never connects.
        'DATABASE_URL': 'postgresql+asyncpg://build_dev:unused@127.0.0.1:1/build',
        'SMTP_USERNAME': 'test@example.test', 'SMTP_PASSWORD': 'disposable',
        'SMTP_FROM_ADDRESS': 'test@example.test',
    }.items():
        monkeypatch.setenv(key, value)
    monkeypatch.chdir(tmp_path)
    result = run_runtime_probe('metadata')
    assert result.returncode == 0, result.stdout + result.stderr
