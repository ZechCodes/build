"""Exercise the operator boundary against a disposable SQLite database."""

import asyncio
import json
from types import SimpleNamespace
from uuid import uuid4

import pytest
from skrift.db.models.user import User
from sqlalchemy import select

from buildapp.db_test_support import (
    add_account,
    create_skrift_tables,
    engine_for,
    file_session_maker,
)
from buildapp.models import Device


ACTOR = "kubernetes:operator@example.com"


@pytest.fixture
def policy_db(tmp_path, monkeypatch):
    database = tmp_path / "policy.db"
    maker = file_session_maker(database)
    engine = engine_for(maker)
    config = SimpleNamespace(
        auth=SimpleNamespace(redirect_base_url="https://dev.getbuild.ing"),
        db=SimpleNamespace(url=f"sqlite+aiosqlite:///{database}", db_schema=None),
    )
    monkeypatch.setattr("skrift.config.get_settings", lambda: config)
    asyncio.run(create_skrift_tables(engine))
    yield maker, config
    asyncio.run(engine.dispose())


async def seed(maker, *, active=True, offline_device=False):
    async with maker() as db:
        user_id = await add_account(db, f"{uuid4()}@example.com")
        user = await db.get(User, user_id)
        user.is_active = active
        if offline_device:
            db.add(Device(
                owner_user_id=user_id, name="offline", approved=True,
                identity_public_key_b64="identity", transport_public_key_b64="transport",
            ))
        await db.commit()
        return user_id


def run_set(user_id, *options, env="dev"):
    from buildapp.account_policy import main

    return main(["set", "--env", env, "--user-id", str(user_id), "--actor", ACTOR, *options])


def values(account_type="free", tunnel_access=False, promotion_revision=0):
    return {
        "account_type": account_type,
        "tunnel_access": tunnel_access,
        "promotion_revision": promotion_revision,
    }


async def snapshot(maker, user_id):
    from dataclasses import asdict

    from buildapp.accounts import account_policy
    from buildapp.models import AccountPolicyAudit

    async with maker() as db:
        policy = asdict(await account_policy(db, user_id))
        audits = (await db.scalars(
            select(AccountPolicyAudit).order_by(AccountPolicyAudit.created_at)
        )).all()
        return policy, audits


def test_account_policy_independent_updates_revision_audit_and_offline_pairing(policy_db):
    maker, _ = policy_db
    user = asyncio.run(seed(maker, offline_device=True))
    assert asyncio.run(snapshot(maker, user)) == (values(), [])
    assert run_set(user, "--type", "admin") == 0
    assert asyncio.run(snapshot(maker, user))[0] == values("admin")
    assert run_set(user, "--tunnel-access", "on") == 0
    assert asyncio.run(snapshot(maker, user))[0] == values("admin", True, 1)
    assert run_set(user, "--tunnel-access", "on") == 0
    assert run_set(user, "--type", "free") == 0
    assert asyncio.run(snapshot(maker, user))[0] == values("free", True, 1)
    assert run_set(user, "--tunnel-access", "off") == 0
    assert asyncio.run(snapshot(maker, user))[0] == values("free", False, 1)
    assert run_set(user, "--tunnel-access", "on") == 0
    policy, audits = asyncio.run(snapshot(maker, user))
    assert policy == values("free", True, 2)
    assert [(row.actor, row.target_user_id) for row in audits] == [(ACTOR, user)] * 5
    assert [(row.old_policy, row.new_policy) for row in audits] == [
        (values(), values("admin")),
        (values("admin"), values("admin", True, 1)),
        (values("admin", True, 1), values("free", True, 1)),
        (values("free", True, 1), values("free", False, 1)),
        (values("free", False, 1), values("free", True, 2)),
    ]


def test_account_policy_dry_run_and_default_noop_do_not_persist(policy_db, capsys):
    maker, _ = policy_db
    user = asyncio.run(seed(maker))
    assert run_set(user, "--type", "admin", "--tunnel-access", "on", "--dry-run") == 0
    report = json.loads(capsys.readouterr().out)
    assert report == {
        "environment": "dev", "actor": ACTOR, "user_id": str(user), "dry_run": True,
        "changed": True, "old_policy": values(), "new_policy": values("admin", True, 1),
    }
    assert run_set(user, "--type", "free", "--tunnel-access", "off") == 0
    assert json.loads(capsys.readouterr().out)["changed"] is False
    assert asyncio.run(snapshot(maker, user)) == (values(), [])

    from buildapp.models import AccountPolicy

    async def row():
        async with maker() as db:
            return await db.get(AccountPolicy, user)

    assert asyncio.run(row()) is None


@pytest.mark.parametrize(("user", "options", "env"), [
    ("not-a-uuid", ("--type", "admin"), "dev"),
    (uuid4(), ("--type", "admin"), "prod"),
    (uuid4(), (), "dev"),
])
def test_account_policy_rejects_bad_cli_input_before_consulting_config(
    monkeypatch, user, options, env,
):
    def unexpected_config():
        pytest.fail("invalid arguments reached database configuration")

    monkeypatch.setattr("skrift.config.get_settings", unexpected_config)
    with pytest.raises(SystemExit) as exit_info:
        run_set(user, *options, env=env)
    assert exit_info.value.code == 2


@pytest.mark.parametrize("unsafe_field", ["issuer", "database"])
def test_account_policy_refuses_wrong_environment_before_connecting(
    policy_db, monkeypatch, capsys, unsafe_field,
):
    maker, config = policy_db
    user = asyncio.run(seed(maker))
    if unsafe_field == "issuer":
        config.auth.redirect_base_url = "https://getbuild.ing"
    else:
        config.db.url = "postgresql+asyncpg://build@localhost/build"

    def unexpected_connection(*args):
        pytest.fail("invalid environment opened a database engine")

    monkeypatch.setattr("buildapp.account_policy.dev_engine", unexpected_connection)
    assert run_set(user, "--tunnel-access", "on") == 1
    assert "requires hosted dev" in capsys.readouterr().err


def test_account_policy_refuses_unknown_and_suspended_promotions(policy_db, capsys):
    maker, _ = policy_db
    foreign = uuid4()
    suspended = asyncio.run(seed(maker, active=False))
    for user in (foreign, suspended):
        assert run_set(user, "--tunnel-access", "on") == 1
        assert "active account" in capsys.readouterr().err
        assert asyncio.run(snapshot(maker, user)) == (values(), [])


def test_account_policy_can_withdraw_access_from_suspended_account(policy_db):
    maker, _ = policy_db
    user = asyncio.run(seed(maker))
    assert run_set(user, "--tunnel-access", "on") == 0

    async def suspend():
        async with maker() as db:
            (await db.get(User, user)).is_active = False
            await db.commit()

    asyncio.run(suspend())
    assert run_set(user, "--tunnel-access", "off") == 0
    assert asyncio.run(snapshot(maker, user))[0] == values("free", False, 1)


def test_account_policy_concurrent_independent_updates_are_both_kept(policy_db):
    from buildapp.account_policy import set_policy

    maker, config = policy_db
    user = asyncio.run(seed(maker))

    async def update_both():
        await asyncio.gather(
            set_policy(config.db.url, user, ACTOR, account_type="admin"),
            set_policy(config.db.url, user, ACTOR, tunnel_access=True),
        )

    asyncio.run(update_both())
    policy, audits = asyncio.run(snapshot(maker, user))
    assert policy == values("admin", True, 1)
    assert len(audits) == 2
    assert audits[0].old_policy == values()
    assert audits[1].old_policy == audits[0].new_policy
    assert audits[1].new_policy == policy
