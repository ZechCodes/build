"""Set Build account policy with the hosted-dev operator's database access."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
from dataclasses import asdict
from uuid import UUID

from skrift.db.models.user import User
from sqlalchemy import select, text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from buildapp.accounts import AccountPolicyValues, account_policy
from buildapp.dev_bootstrap import dev_database_url, dev_engine, verify_dev_connection
from buildapp.models import AccountPolicy, AccountPolicyAudit


class PolicyChangeRefused(Exception):
    """An operator request that is invalid for the target account."""


async def account_for_change(
    db: AsyncSession, user_id: UUID, tunnel_access: bool | None, dry_run: bool,
) -> None:
    # Lock the existing user even when there is no policy row. SQLite has no
    # row lock, so the offline operator fixture reserves its write transaction.
    statement = select(User.is_active).where(User.id == user_id)
    if not dry_run:
        if db.bind.dialect.name == "sqlite":
            await db.execute(text("BEGIN IMMEDIATE"))
        statement = statement.with_for_update()
    active = await db.scalar(statement)
    if active is None or (not active and tunnel_access is not False):
        raise PolicyChangeRefused("policy change requires an active account in the selected issuer")


async def set_policy(
    url: str, user_id: UUID, actor: str, *, account_type: str | None = None,
    tunnel_access: bool | None = None, dry_run: bool = False,
) -> dict:
    engine = dev_engine(url)
    try:
        async with async_sessionmaker(engine)() as db:
            await verify_dev_connection(await db.connection())
            await account_for_change(db, user_id, tunnel_access, dry_run)
            old = await account_policy(db, user_id)
            new = updated_policy(old, account_type, tunnel_access)
            report = {
                "environment": "dev", "actor": actor, "user_id": str(user_id), "dry_run": dry_run,
                "changed": old != new, "old_policy": asdict(old), "new_policy": asdict(new),
            }
            if old != new and not dry_run:
                await db.merge(AccountPolicy(id=user_id, **report["new_policy"]))
                db.add(AccountPolicyAudit(
                    target_user_id=user_id, actor=actor,
                    old_policy=report["old_policy"], new_policy=report["new_policy"],
                ))
                await db.commit()
            return report
    finally:
        await engine.dispose()


def updated_policy(
    old: AccountPolicyValues, account_type: str | None, tunnel_access: bool | None,
) -> AccountPolicyValues:
    return AccountPolicyValues(
        account_type=old.account_type if account_type is None else account_type,
        tunnel_access=old.tunnel_access if tunnel_access is None else tunnel_access,
        promotion_revision=old.promotion_revision + int(tunnel_access is True and not old.tunnel_access),
    )


def operator_actor(value: str) -> str:
    if not value or len(value) > 512 or not value.isprintable() or value != value.strip():
        raise argparse.ArgumentTypeError("invalid operator actor")
    return value


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    commands = parser.add_subparsers(dest="command", required=True)
    change = commands.add_parser("set", allow_abbrev=False)
    change.add_argument("--env", required=True, choices=("dev",))
    change.add_argument("--user-id", required=True, type=UUID)
    change.add_argument("--type", dest="account_type", choices=("free", "admin"))
    change.add_argument("--tunnel-access", choices=("on", "off"))
    change.add_argument("--dry-run", action="store_true")
    change.add_argument("--actor", type=operator_actor, default=f"uid:{os.geteuid()}")
    args = parser.parse_args(argv)
    if args.account_type is None and args.tunnel_access is None:
        change.error("supply --type or --tunnel-access")
    return args


def main(argv=None) -> int:
    args = parse_args(argv)
    tunnel_access = {"on": True, "off": False, None: None}[args.tunnel_access]
    try:
        url = dev_database_url()
        report = asyncio.run(set_policy(
            url, args.user_id, args.actor, account_type=args.account_type,
            tunnel_access=tunnel_access, dry_run=args.dry_run,
        ))
    except (RuntimeError, PolicyChangeRefused) as error:
        print(str(error), file=sys.stderr)
        return 1
    except Exception:
        print("account policy operation failed; inspect the operator environment", file=sys.stderr)
        return 1
    print(json.dumps(report))
    return 0


if __name__ == "__main__":
    sys.exit(main())
