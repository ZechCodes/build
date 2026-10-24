"""Cold-process hosted-dev ORM probes, using only disposable test databases.

Do not import models before create_app: that would hide Skrift's schema/import
ordering bug by resolving foreign keys before it changes metadata.schema.
"""
from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys


def run_runtime_probe(mode: str) -> subprocess.CompletedProcess[str]:
    environment = dict(os.environ, PYTHONPATH=str(Path(__file__).resolve().parents[1]))
    return subprocess.run(
        [sys.executable, '-m', 'buildapp.dev_runtime_test_support', mode],
        env=environment, capture_output=True, text=True, timeout=60,
    )


def check_metadata() -> None:
    from skrift.db.base import Base

    # Force every target lookup, including tables not touched by health checks.
    for table in Base.metadata.tables.values():
        for foreign_key in table.foreign_keys:
            assert foreign_key.column.table.metadata is Base.metadata


def on_database(client, work):
    from skrift.config import get_settings
    from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine
    from sqlalchemy.pool import NullPool

    async def run():
        db = get_settings().db
        options = {'schema_translate_map': {None: db.db_schema}} if db.db_schema else {}
        engine = create_async_engine(db.url, poolclass=NullPool, execution_options=options)
        try:
            async with async_sessionmaker(engine, expire_on_commit=False)() as session:
                return await work(session)
        finally:
            await engine.dispose()

    with client.portal() as portal:
        return portal.call(run)


def check_device(client) -> None:
    from skrift.db.models.user import User
    from sqlalchemy import select
    from buildapp.models import Device

    async def write(session):
        user = User(email='device-proof@example.test', name='Disposable device owner')
        session.add(user)
        await session.commit()
        device = Device(name='Disposable device', owner_user_id=user.id,
                        identity_public_key_b64='disposable-identity',
                        transport_public_key_b64='disposable-transport')
        session.add(device)
        await session.flush()
        await session.commit()
        return device.id, user.id

    device_id, user_id = on_database(client, write)

    async def read(session):
        assert await session.scalar(select(Device.owner_user_id).where(Device.id == device_id)) == user_id

    on_database(client, read)


def check_signup(client) -> None:
    from types import SimpleNamespace
    from unittest.mock import patch
    from skrift.db.models.second_factor import SecondFactorEnrollment
    from skrift.db.models.user import User
    from sqlalchemy import insert, select
    from buildapp.clock import utc_now
    from buildapp.invites import INVITE_TTL, invite_path, mint_invite_token
    from buildapp.token_hash import token_hash
    from buildapp.models import Invite, UserEmailPreference
    from buildapp.test_invite_signup import Page

    async def issue(session):
        from uuid import uuid4
        invite_id, raw, now = uuid4(), mint_invite_token(), utc_now()
        # Seed through Core so this fixture does not warm ORM FK resolution
        # before the registration transaction we are testing.
        await session.execute(insert(Invite.__table__).values(
            id=invite_id, token_hash=token_hash(raw), kind='open_link',
            email='', expires_at=now + INVITE_TTL,
        ))
        await session.commit()
        return invite_id, raw

    invite_id, raw = on_database(client, issue)
    assert client.get(invite_path(raw), follow_redirects=False).status_code == 302
    verified = SimpleNamespace(credential_id='credential-1', public_key='disposable-public-key',
                               sign_count=0, transports=['internal'], enrollment_metadata={})
    with patch('skrift.controllers.auth.complete_primary_passkey_registration', return_value=verified):
        response = Page(client).create_account('signup-proof@example.test', opt_in='on')
    assert response.status_code == 201, response.text
    assert response.json() == {'ok': True, 'redirect': '/app/'}

    async def read(session):
        user = await session.scalar(select(User).where(User.email == 'signup-proof@example.test'))
        assert user is not None
        enrollment = await session.scalar(select(SecondFactorEnrollment).where(
            SecondFactorEnrollment.user_id == user.id))
        assert enrollment is not None and enrollment.credential_id == 'credential-1'
        invite = await session.get(Invite, invite_id)
        assert invite.redeemed_by == user.id and invite.redeemed_at is not None
        preference = await session.scalar(select(UserEmailPreference).where(
            UserEmailPreference.user_id == user.id))
        assert preference is not None and preference.product_email_opt_in
        assert preference.product_email_opted_in_at is not None

    on_database(client, read)


def main(mode: str) -> None:
    from skrift.asgi import create_app
    app = create_app()  # The real runtime's first model import; no bootstrap imports.
    if mode == 'metadata':
        check_metadata()
        return
    from litestar.testing import TestClient
    from skrift.config import get_settings
    from sqlalchemy.engine import make_url

    parsed = make_url(get_settings().db.url)
    assert parsed.host in ('localhost', '127.0.0.1', '::1')
    assert parsed.username == 'build_dev' and parsed.database == 'build' and not parsed.query
    with TestClient(app, base_url='https://dev.getbuild.ing') as client:
        {'device': check_device, 'signup': check_signup}[mode](client)


if __name__ == '__main__':
    main(sys.argv[1])
