"""Operator-only hosted-dev first boot; never called by the production entrypoint.

Migrations come first. Persist the private invite before closing setup, so a failed
credential write cannot authorize the first app rollout. No raw token is logged.
"""
from __future__ import annotations

import argparse
import asyncio
import os
from pathlib import Path
import subprocess
import sys

from skrift.db.models.setting import Setting
from skrift.db.models.user import User
from skrift.db.services.setting_service import SETUP_COMPLETED_AT_KEY, get_setting
from sqlalchemy import select
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from buildapp.clock import utc_now
from buildapp.invites import invite_url, issue_open_invite
from buildapp.models import Invite

DEV_ORIGIN = 'https://dev.getbuild.ing'
DEV_DATABASE_HOST = 'build-postgres.build-dev.svc.cluster.local'


def dev_database_url() -> str:
    """Reject production configuration before opening a database connection."""
    from skrift.config import get_settings

    settings = get_settings()
    url = make_url(settings.db.url)
    if settings.auth.redirect_base_url != DEV_ORIGIN:
        raise RuntimeError('requires hosted dev configuration')
    # SQLite files support disposable offline proof. Hosted Postgres must be the
    # exact dev service, never a production URL or an arbitrary override.
    if url.drivername != 'sqlite+aiosqlite' and not (
        url.drivername == 'postgresql+asyncpg' and url.host == DEV_DATABASE_HOST
        and url.database == 'build' and url.username == 'build'
        and url.port in (None, 5432) and not url.query
    ):
        raise RuntimeError('requires hosted dev database')
    return settings.db.url


async def require_fresh(session) -> None:
    for model in (User, Invite):
        if await session.scalar(select(model.id).limit(1)) is not None:
            raise RuntimeError('bootstrap requires a fresh database')
    if await get_setting(session, SETUP_COMPLETED_AT_KEY) is not None:
        raise RuntimeError('bootstrap requires a fresh database')


async def bootstrap(url: str, invite_file: Path) -> None:
    """One operator, one fresh DB, one exclusive private output file.

    A partial failure is intentionally not retried: keep ingress absent and
    inspect/recreate the disposable database rather than mint extra live links.
    """
    engine = create_async_engine(url)
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as session:
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
    engine = create_async_engine(url)
    try:
        async with async_sessionmaker(engine)() as session:
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
    commands.add_parser('migrate-check')
    args = parser.parse_args()
    try:
        url = dev_database_url()
        if args.command == 'migrate-check':
            subprocess.run(['skrift', 'db', 'upgrade', 'heads'], check=True)
        if args.command == 'bootstrap':
            asyncio.run(bootstrap(url, args.invite_file))
        else:
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
