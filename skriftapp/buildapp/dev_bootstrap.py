"""Operator-only hosted-dev first boot; never called by the production entrypoint.

Migrations come first. Persist the private invite before closing setup, so a failed
credential write cannot authorize the first app rollout. No raw token is logged.
"""
from __future__ import annotations

import argparse
import asyncio
import os
from pathlib import Path
import sys

from skrift.db.models.setting import Setting
from skrift.db.models.user import User
from skrift.db.services.setting_service import SETUP_COMPLETED_AT_KEY, get_setting
from sqlalchemy import select, text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine
from sqlalchemy.pool import NullPool

from buildapp.clock import utc_now
from buildapp.invites import invite_url, issue_open_invite
from buildapp.models import Invite

DEV_ORIGIN = 'https://dev.getbuild.ing'
DEV_DATABASE_HOST = 'build-postgres.8ly.svc.cluster.local'
DEV_DATABASE_ROLE = DEV_DATABASE_SCHEMA = 'build_dev'


def dev_database_url() -> str:
    """Reject production configuration before opening a database connection."""
    from skrift.config import get_settings

    settings = get_settings()
    url = make_url(settings.db.url)
    if settings.auth.redirect_base_url != DEV_ORIGIN:
        raise RuntimeError('requires hosted dev configuration')
    # SQLite files support disposable offline proof. Hosted Postgres must be the
    # shared service with its dedicated dev role/schema, never the prod role.
    offline = url.drivername == 'sqlite+aiosqlite' and settings.db.db_schema is None
    if not offline and not is_hosted_dev_url(url, settings.db.db_schema):
        raise RuntimeError('requires hosted dev database')
    return settings.db.url


def is_hosted_dev_url(url, schema: str | None) -> bool:
    return (url.drivername == 'postgresql+asyncpg' and url.host == DEV_DATABASE_HOST
            and url.database == 'build' and url.username == DEV_DATABASE_ROLE
            and schema is None
            and url.port in (None, 5432) and not url.query)


def dev_engine(url: str):
    """Operator commands use one connection, without holding idle pool slots."""
    options = {} if make_url(url).drivername == 'sqlite+aiosqlite' else {
        'schema_translate_map': {None: DEV_DATABASE_SCHEMA},
    }
    return create_async_engine(url, poolclass=NullPool, execution_options=options)


async def verify_dev_connection(connection) -> None:
    """Validate the effective backend identity before any migration or app query."""
    if connection.dialect.name == 'sqlite':
        return
    safe = await connection.scalar(text("""
        SELECT current_user = :role AND session_user = :role
          AND current_database() = 'build'
          AND current_schema() = :schema
          AND current_setting('search_path') = :schema
          AND n.nspowner = r.oid
          AND NOT (r.rolsuper OR r.rolcreatedb OR r.rolcreaterole
                   OR r.rolreplication OR r.rolbypassrls)
          AND NOT EXISTS (SELECT 1 FROM pg_auth_members WHERE member = r.oid)
          AND NOT EXISTS (
            SELECT 1 FROM pg_namespace other
            WHERE other.nspname <> :schema AND has_schema_privilege(other.oid, 'CREATE'))
          AND NOT EXISTS (
            SELECT 1 FROM pg_class c JOIN pg_namespace p ON p.oid = c.relnamespace
            WHERE p.nspname !~ '^pg_' AND p.nspname <> 'information_schema'
              AND p.nspname <> :schema AND (
                (c.relkind IN ('r', 'p', 'v', 'm', 'f') AND
                  (has_table_privilege(c.oid, 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
                   OR has_any_column_privilege(c.oid, 'SELECT,INSERT,UPDATE,REFERENCES')))
                OR (c.relkind = 'S' AND has_sequence_privilege(c.oid, 'USAGE,SELECT,UPDATE'))))
        FROM pg_roles r JOIN pg_namespace n ON n.nspname = :schema
        WHERE r.rolname = current_user
    """), {'role': DEV_DATABASE_ROLE, 'schema': DEV_DATABASE_SCHEMA})
    if not safe:
        raise RuntimeError('requires isolated hosted dev database identity and schema')


def upgrade_on_connection(connection) -> None:
    """Run the installed real revision graph on this already-checked connection."""
    import skrift
    from alembic import command
    from alembic.config import Config

    config = Config()
    config.set_main_option('script_location', str(Path(__file__).with_name('dev_migrations')))
    config.set_main_option('path_separator', 'os')
    revisions = [Path(__file__).resolve().parents[1] / 'migrations/versions',
                 Path(skrift.__file__).parent / 'alembic/versions']
    config.set_main_option('version_locations', os.pathsep.join(map(str, revisions)))
    config.attributes['connection'] = connection
    config.attributes['schema'] = None if connection.dialect.name == 'sqlite' else DEV_DATABASE_SCHEMA
    command.upgrade(config, 'heads')


async def migrate(url: str) -> None:
    engine = dev_engine(url)
    try:
        async with engine.begin() as connection:
            await verify_dev_connection(connection)
            await connection.run_sync(upgrade_on_connection)
    finally:
        await engine.dispose()


async def require_fresh(session) -> None:
    for model in (User, Invite):
        if await session.scalar(select(model.id).limit(1)) is not None:
            raise RuntimeError('bootstrap requires a fresh database')
    if await get_setting(session, SETUP_COMPLETED_AT_KEY) is not None:
        raise RuntimeError('bootstrap requires a fresh database')


async def bootstrap(url: str, invite_file: Path) -> None:
    """One operator, one fresh dev schema, one exclusive private output file.

    A partial failure is intentionally not retried: keep ingress absent and
    inspect operator state rather than mint extra live links or reset the DB.
    """
    engine = dev_engine(url)
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as session:
            await verify_dev_connection(await session.connection())
            await require_fresh(session)
            descriptor = os.open(invite_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, 'w') as output:
                _, raw = await issue_open_invite(session, None, utc_now())
                output.write(invite_url(DEV_ORIGIN, raw) + '\n')
                output.flush()
                os.fsync(output.fileno())
            # Last write authorizes deployment only after private token storage.
            session.add(Setting(key=SETUP_COMPLETED_AT_KEY, value=utc_now().isoformat()))
            await session.commit()
    finally:
        await engine.dispose()


async def check_bootstrapped(url: str) -> None:
    """Read-only rollout gate; consumed/expired invites still prove prior seeding."""
    engine = dev_engine(url)
    try:
        async with async_sessionmaker(engine)() as session:
            await verify_dev_connection(await session.connection())
            marker = await get_setting(session, SETUP_COMPLETED_AT_KEY)
            invite = await session.scalar(select(Invite.id).limit(1))
            if not marker or invite is None:
                raise RuntimeError('operator bootstrap required before app exposure')
    finally:
        await engine.dispose()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    seed = commands.add_parser('bootstrap')
    seed.add_argument('--invite-file', required=True, type=Path)
    commands.add_parser('check')
    commands.add_parser('migrate')
    commands.add_parser('migrate-check')
    args = parser.parse_args()
    try:
        url = dev_database_url()
        if args.command in ('migrate', 'migrate-check'):
            asyncio.run(migrate(url))
        if args.command == 'bootstrap':
            asyncio.run(bootstrap(url, args.invite_file))
        elif args.command != 'migrate':
            asyncio.run(check_bootstrapped(url))
    except RuntimeError as error:
        print(str(error), file=sys.stderr)
        return 1
    except Exception:
        # Database/OS exception strings may contain credentials. No traceback in
        # pod/CI logs; an operator investigates with the public app still absent.
        print('dev bootstrap failed; keep app unexposed and inspect operator state', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
