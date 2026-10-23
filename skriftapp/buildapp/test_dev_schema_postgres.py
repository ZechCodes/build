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
