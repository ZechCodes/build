"""Optional real PG16 isolation proof against an operator-created disposable DB.

TEST_DEV_POSTGRES_URL must name a loopback-only disposable database with role
build_dev, dev-only search_path, and an inaccessible public.prod_canary table.
Never targets the cluster. Normal unit gates skip when no local proof DB exists.
"""
import asyncio
import os
from pathlib import Path
import stat

import pytest
from sqlalchemy import text
from sqlalchemy.engine import make_url
from sqlalchemy.exc import DBAPIError
import yaml

ROOT = Path(__file__).resolve().parents[2]


def test_real_postgres_schema_migrations_bootstrap_and_app(tmp_path, monkeypatch):
    url = os.environ.get('TEST_DEV_POSTGRES_URL')
    if not url:
        pytest.skip('requires disposable local PostgreSQL16')
    parsed = make_url(url)
    assert parsed.drivername == 'postgresql+asyncpg' and not parsed.query
    assert parsed.host in ('localhost', '127.0.0.1', '::1')
    assert parsed.database == 'build' and parsed.username == 'build_dev'
    config = yaml.safe_load((ROOT / 'skriftapp/app.channel-dev.yaml').read_text())
    (tmp_path / 'app.yaml').write_text(yaml.safe_dump(config))
    (tmp_path / 'templates').symlink_to(ROOT / 'skriftapp/templates')
    for key, value in {
        'SKRIFT_ENV': 'production', 'SECRET_KEY': 'disposable-postgres-proof-secret',
        'DATABASE_URL': url, 'SMTP_USERNAME': 'test@example.com',
        'SMTP_PASSWORD': 'disposable', 'SMTP_FROM_ADDRESS': 'test@example.com',
    }.items():
        monkeypatch.setenv(key, value)
    monkeypatch.chdir(tmp_path)
    from skrift.config import get_settings
    from skrift.db.base import Base
    from buildapp.dev_bootstrap import bootstrap, check_bootstrapped, dev_engine, migrate, verify_dev_connection
    get_settings.cache_clear()
    prior_schema = Base.metadata.schema
    output = tmp_path / 'private-invite.txt'

    async def prove():
        engine = dev_engine(url)
        try:
            async with engine.connect() as connection:
                assert await connection.scalar(text('SHOW server_version_num')) >= '160000'
                assert await connection.scalar(text("SELECT count(*) FROM pg_tables WHERE schemaname='build_dev'")) == 0
                await connection.execute(text('SET search_path = public'))
                with pytest.raises(RuntimeError, match='identity and schema'):
                    await verify_dev_connection(connection)
            await migrate(url)
            await migrate(url)  # Both branches and their version records are idempotent.
            with pytest.raises(RuntimeError, match='bootstrap'):
                await check_bootstrapped(url)
            await bootstrap(url, output)
            await check_bootstrapped(url)
            assert stat.S_IMODE(output.stat().st_mode) == 0o600
            assert output.read_text().startswith('https://dev.getbuild.ing/invite/inv_')
            with pytest.raises(RuntimeError, match='fresh'):
                await bootstrap(url, tmp_path / 'second.txt')
            async with engine.connect() as connection:
                versions = (await connection.execute(text('SELECT version_num FROM build_dev.alembic_version'))).scalars().all()
                assert len(versions) == 2
                assert not await connection.scalar(text("SELECT EXISTS (SELECT 1 FROM pg_tables WHERE schemaname='build_dev' AND tableowner <> 'build_dev')"))
                assert not await connection.scalar(text("SELECT EXISTS (SELECT 1 FROM pg_constraint fk JOIN pg_class t ON t.oid=fk.conrelid JOIN pg_namespace n ON n.oid=t.relnamespace JOIN pg_class target ON target.oid=fk.confrelid JOIN pg_namespace dest ON dest.oid=target.relnamespace WHERE fk.contype='f' AND n.nspname='build_dev' AND dest.nspname <> 'build_dev')"))
                assert await connection.scalar(text("SELECT count(*) FROM pg_tables WHERE schemaname='build_dev'")) > 20
                assert await connection.scalar(text("SELECT count(*) FROM pg_tables WHERE schemaname='public'")) == 1
            for forbidden in ('SELECT * FROM public.prod_canary',
                              "UPDATE public.prod_canary SET value='changed'",
                              'CREATE TABLE public.dev_escape (id integer)',
                              'SET ROLE build'):
                async with engine.connect() as connection:
                    with pytest.raises(DBAPIError):
                        await connection.execute(text(forbidden))
        finally:
            await engine.dispose()
    try:
        asyncio.run(prove())
        from litestar.testing import TestClient
        from skrift.asgi import create_dispatcher
        with TestClient(create_dispatcher(), base_url='https://dev.getbuild.ing') as client:
            for route in ('/healthz', '/readyz', '/auth/login'):
                assert client.get(route, follow_redirects=False).status_code == 200
            assert client.get('/setup', follow_redirects=False).status_code == 404
    finally:
        Base.metadata.schema = prior_schema
        get_settings.cache_clear()


@pytest.fixture
def disposable_postgres_connections():
    """Privileged fixture is opt-in and must share the dev proof's local server."""
    from buildapp.db_test_support import postgres_proof_engine
    from buildapp.dev_bootstrap import dev_engine

    dev_url = os.environ.get('TEST_DEV_POSTGRES_URL')
    admin_url = os.environ.get('TEST_DEV_POSTGRES_ADMIN_URL')
    if not dev_url or not admin_url:
        pytest.skip('requires disposable local PostgreSQL16 dev and admin URLs')
    dev, admin = make_url(dev_url), make_url(admin_url)
    for parsed in (dev, admin):
        assert parsed.drivername == 'postgresql+asyncpg' and not parsed.query
        assert parsed.host in ('localhost', '127.0.0.1', '::1')
        assert parsed.database == 'build'
    assert dev.username == 'build_dev' and admin.username == 'postgres'
    assert (dev.host, dev.port) == (admin.host, admin.port)
    return dev_engine(dev_url), postgres_proof_engine(admin_url)


@pytest.mark.parametrize('privilege,grant,revoke', [
    ('SELECT', text('GRANT SELECT (value) ON public.prod_canary TO build_dev'),
     text('REVOKE SELECT (value) ON public.prod_canary FROM build_dev')),
    ('INSERT', text('GRANT INSERT (value) ON public.prod_canary TO build_dev'),
     text('REVOKE INSERT (value) ON public.prod_canary FROM build_dev')),
    ('UPDATE', text('GRANT UPDATE (value) ON public.prod_canary TO build_dev'),
     text('REVOKE UPDATE (value) ON public.prod_canary FROM build_dev')),
    ('REFERENCES', text('GRANT REFERENCES (value) ON public.prod_canary TO build_dev'),
     text('REVOKE REFERENCES (value) ON public.prod_canary FROM build_dev')),
])
def test_preflight_rejects_column_only_production_privilege(disposable_postgres_connections, privilege, grant, revoke):
    from buildapp.dev_bootstrap import verify_dev_connection
    dev, admin = disposable_postgres_connections

    async def prove():
        try:
            async with admin.begin() as connection:
                await connection.execute(grant)
            async with dev.connect() as connection:
                assert not await connection.scalar(text(
                    'SELECT has_table_privilege(\'public.prod_canary\', :privilege)'),
                    {'privilege': privilege})
                with pytest.raises(RuntimeError, match='identity and schema'):
                    await verify_dev_connection(connection)
        finally:
            async with admin.begin() as connection:
                await connection.execute(revoke)
            await dev.dispose()
            await admin.dispose()
    asyncio.run(prove())


@pytest.mark.parametrize('grant,revoke', [
    pytest.param(text('GRANT CREATE ON SCHEMA public TO build_dev'),
                 text('REVOKE CREATE ON SCHEMA public FROM build_dev'), id='public'),
    pytest.param(text('GRANT CREATE ON SCHEMA unrelated_app TO build_dev'),
                 text('REVOKE CREATE ON SCHEMA unrelated_app FROM build_dev'), id='unrelated_app'),
    pytest.param(text('GRANT CREATE ON SCHEMA pg_catalog TO build_dev'),
                 text('REVOKE CREATE ON SCHEMA pg_catalog FROM build_dev'), id='pg_catalog'),
    pytest.param(text('GRANT CREATE ON SCHEMA information_schema TO build_dev'),
                 text('REVOKE CREATE ON SCHEMA information_schema FROM build_dev'), id='information_schema'),
])
def test_preflight_rejects_create_on_every_other_schema(disposable_postgres_connections, grant, revoke):
    from buildapp.dev_bootstrap import verify_dev_connection
    dev, admin = disposable_postgres_connections

    async def prove():
        try:
            async with admin.begin() as connection:
                await connection.execute(text('CREATE SCHEMA IF NOT EXISTS unrelated_app'))
                await connection.execute(grant)
            async with dev.connect() as connection:
                with pytest.raises(RuntimeError, match='identity and schema'):
                    await verify_dev_connection(connection)
        finally:
            async with admin.begin() as connection:
                await connection.execute(revoke)
                await connection.execute(text('DROP SCHEMA unrelated_app'))
            await dev.dispose()
            await admin.dispose()
    asyncio.run(prove())
