"""Fresh hosted-dev boot crosses the real migration/serving boundary."""
import asyncio
from pathlib import Path
import stat
import subprocess
import sys

import pytest
import yaml
from buildapp.db_test_support import engine_for, file_session_maker

ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture
def fresh_hosted_dev(tmp_path, monkeypatch):
    config = yaml.safe_load((ROOT / 'skriftapp/app.channel-dev.yaml').read_text())
    (tmp_path / 'app.yaml').write_text(yaml.safe_dump(config))
    for name in ('templates', 'migrations'):
        (tmp_path / name).symlink_to(ROOT / 'skriftapp' / name)
    url = f"sqlite+aiosqlite:///{tmp_path / 'fresh.db'}"
    for key, value in {
        'SKRIFT_ENV': 'production', 'SECRET_KEY': 'disposable-bootstrap-session-secret',
        'DATABASE_URL': url, 'SMTP_USERNAME': 'test@example.com',
        'SMTP_PASSWORD': 'disposable', 'SMTP_FROM_ADDRESS': 'test@example.com',
        'PYTHONPATH': str(ROOT / 'skriftapp'),
    }.items():
        monkeypatch.setenv(key, value)
    monkeypatch.chdir(tmp_path)
    result = subprocess.run([str(Path(sys.executable).with_name('skrift')), 'db', 'upgrade', 'heads'],
                            capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    from skrift.config import get_settings
    get_settings.cache_clear()
    yield url, tmp_path
    get_settings.cache_clear()


def test_fresh_migrated_dispatcher_bootstraps(fresh_hosted_dev, capsys):
    from litestar.testing import TestClient
    from skrift.asgi import create_dispatcher
    from buildapp.dev_bootstrap import bootstrap, check_bootstrapped
    from buildapp.invites import INVITE_TTL, find_by_token
    from buildapp.clock import utc_now

    url, directory = fresh_hosted_dev
    with TestClient(create_dispatcher(), base_url='https://dev.getbuild.ing') as client:
        assert client.get('/healthz', follow_redirects=False).headers['location'] == '/setup'
    with pytest.raises(RuntimeError, match='bootstrap'):
        asyncio.run(check_bootstrapped(url))
    output = directory / 'invite.txt'
    asyncio.run(bootstrap(url, output))
    assert stat.S_IMODE(output.stat().st_mode) == 0o600
    link = output.read_text().strip()
    assert link.startswith('https://dev.getbuild.ing/invite/inv_')
    raw = link.rsplit('/', 1)[-1]
    captured = capsys.readouterr()
    assert raw not in captured.out + captured.err

    async def inspect_invite():
        maker = file_session_maker(directory / 'fresh.db')
        try:
            async with maker() as session:
                invite = await find_by_token(session, raw)
                assert invite and invite.token_hash != raw
                assert invite.redeemed_at is None and invite.invited_by is None
                assert abs((invite.expires_at - utc_now()).total_seconds() - INVITE_TTL.total_seconds()) < 10
        finally:
            await engine_for(maker).dispose()
    asyncio.run(inspect_invite())
    asyncio.run(check_bootstrapped(url))
    with TestClient(create_dispatcher(), base_url='https://dev.getbuild.ing') as client:
        for route in ('/healthz', '/readyz', '/auth/login'):
            assert client.get(route, follow_redirects=False).status_code == 200
        response = client.get('/app/', follow_redirects=False)
        assert response.status_code in (200, 302, 303)
        assert not response.headers.get('location', '').startswith('/setup')
        for route in ('/setup', '/setup/welcome'):
            assert client.get(route, follow_redirects=False).status_code == 404
    with pytest.raises(RuntimeError, match='fresh'):
        asyncio.run(bootstrap(url, directory / 'second.txt'))
    assert not (directory / 'second.txt').exists()


def test_output_collision_never_bootstraps(fresh_hosted_dev):
    from buildapp.dev_bootstrap import bootstrap, check_bootstrapped
    url, directory = fresh_hosted_dev
    target = directory / 'private.txt'
    target.write_text('keep')
    link = directory / 'link.txt'
    link.symlink_to(target)
    for path in (target, link):
        with pytest.raises(FileExistsError):
            asyncio.run(bootstrap(url, path))
        with pytest.raises(RuntimeError, match='bootstrap'):
            asyncio.run(check_bootstrapped(url))
    assert target.read_text() == 'keep'


def test_cli_refuses_prod(fresh_hosted_dev):
    _, directory = fresh_hosted_dev
    (directory / 'app.yaml').write_text((ROOT / 'skriftapp/app.yaml').read_text())
    result = subprocess.run([sys.executable, '-m', 'buildapp.dev_bootstrap', 'bootstrap',
                             '--invite-file', str(directory / 'invite.txt')], capture_output=True, text=True)
    assert result.returncode != 0
    assert 'hosted dev' in result.stderr
    assert not (directory / 'invite.txt').exists()
    assert 'disposable' not in result.stdout + result.stderr


def test_dev_migration_job_checks_bootstrap():
    import yaml
    job = yaml.safe_load((ROOT / 'deploy/k8s/dev/migrate.yaml').read_text())
    container = job['spec']['template']['spec']['containers'][0]
    assert container['command'] == ['python', '-m', 'buildapp.dev_bootstrap', 'migrate-check']


def test_failed_invite_write_keeps_setup_closed(fresh_hosted_dev, monkeypatch):
    from buildapp.dev_bootstrap import bootstrap, check_bootstrapped
    url, directory = fresh_hosted_dev

    def fail_sync(_descriptor):
        raise OSError('simulated disk failure')

    monkeypatch.setattr('buildapp.dev_bootstrap.os.fsync', fail_sync)
    with pytest.raises(OSError):
        asyncio.run(bootstrap(url, directory / 'invite.txt'))
    with pytest.raises(RuntimeError, match='bootstrap'):
        asyncio.run(check_bootstrapped(url))
    with pytest.raises(RuntimeError, match='fresh'):
        asyncio.run(bootstrap(url, directory / 'retry.txt'))


def test_migration_gate_fails_closed_then_passes_without_printing_token(fresh_hosted_dev):
    from buildapp.dev_bootstrap import bootstrap
    url, directory = fresh_hosted_dev
    command = [sys.executable, '-m', 'buildapp.dev_bootstrap', 'migrate-check']
    before = subprocess.run(command, capture_output=True, text=True)
    assert before.returncode == 1
    assert 'operator bootstrap required' in before.stderr
    output = directory / 'invite.txt'
    asyncio.run(bootstrap(url, output))
    raw = output.read_text().strip().rsplit('/', 1)[-1]
    after = subprocess.run(command, capture_output=True, text=True)
    assert after.returncode == 0, after.stderr
    assert raw not in after.stdout + after.stderr


def test_guard_refuses_production_database_before_connecting(fresh_hosted_dev, monkeypatch):
    from skrift.config import get_settings
    from buildapp.dev_bootstrap import dev_database_url
    monkeypatch.setenv('DATABASE_URL', 'postgresql+asyncpg://build:private@build-postgres.prod.svc.cluster.local:5432/build')
    get_settings.cache_clear()
    with pytest.raises(RuntimeError, match='hosted dev database'):
        dev_database_url()


@pytest.mark.parametrize('suffix', [
    '?host=build-postgres.prod.svc.cluster.local:5432', '?database=prod', '?user=prod',
    '?port=5433',
])
def test_guard_refuses_connection_query_overrides(fresh_hosted_dev, monkeypatch, suffix):
    from skrift.config import get_settings
    from buildapp.dev_bootstrap import dev_database_url
    _, directory = fresh_hosted_dev
    config = yaml.safe_load((directory / 'app.yaml').read_text())
    config['db'].pop('schema', None)
    (directory / 'app.yaml').write_text(yaml.safe_dump(config))
    monkeypatch.setenv('DATABASE_URL', 'postgresql+asyncpg://build_dev:private@build-postgres.8ly.svc.cluster.local:5432/build' + suffix)
    get_settings.cache_clear()
    with pytest.raises(RuntimeError, match='hosted dev database'):
        dev_database_url()


def test_guard_accepts_only_dedicated_dev_role_and_schema(fresh_hosted_dev, monkeypatch):
    from skrift.config import get_settings
    from buildapp.dev_bootstrap import dev_database_url
    _, directory = fresh_hosted_dev
    config = yaml.safe_load((directory / 'app.yaml').read_text())
    config['db'].pop('schema', None)
    (directory / 'app.yaml').write_text(yaml.safe_dump(config))
    url = 'postgresql+asyncpg://build_dev:private@build-postgres.8ly.svc.cluster.local:5432/build'
    monkeypatch.setenv('DATABASE_URL', url)
    get_settings.cache_clear()
    assert dev_database_url() == url
    for invalid in (url.replace('build_dev:private', 'build:private'),
                    url + '?options=-csearch_path=public',
                    url.replace('8ly', 'build-dev'),
                    url.replace(':5432/build', ':5432/production'),
                    url.replace(':5432/build', ':5433/build'),
                    url.replace('postgresql+asyncpg', 'postgresql+psycopg')):
        monkeypatch.setenv('DATABASE_URL', invalid)
        get_settings.cache_clear()
        with pytest.raises(RuntimeError, match='hosted dev database'):
            dev_database_url()
    monkeypatch.setenv('DATABASE_URL', url)
    for schema in ('public', 'build_dev'):
        config['db']['schema'] = schema
        (directory / 'app.yaml').write_text(yaml.safe_dump(config))
        get_settings.cache_clear()
        with pytest.raises(RuntimeError, match='hosted dev database'):
            dev_database_url()


def test_operator_migrate_only_does_not_require_or_create_bootstrap(fresh_hosted_dev):
    from buildapp.dev_bootstrap import check_bootstrapped
    url, directory = fresh_hosted_dev
    result = subprocess.run([sys.executable, '-m', 'buildapp.dev_bootstrap', 'migrate'],
                            capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    with pytest.raises(RuntimeError, match='bootstrap'):
        asyncio.run(check_bootstrapped(url))
    assert not list(directory.glob('*.txt'))


def test_sqlite_offline_proof_requires_no_configured_schema(fresh_hosted_dev):
    from skrift.config import get_settings
    from buildapp.dev_bootstrap import dev_database_url
    _, directory = fresh_hosted_dev
    config = yaml.safe_load((directory / 'app.yaml').read_text())
    config['db']['schema'] = 'build_dev'
    (directory / 'app.yaml').write_text(yaml.safe_dump(config))
    get_settings.cache_clear()
    with pytest.raises(RuntimeError, match='hosted dev database'):
        dev_database_url()
