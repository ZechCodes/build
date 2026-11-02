"""Authenticated nonce allocation rejects UUID-only budget consumption."""

import pytest
from litestar.di import Provide
from litestar.exceptions import ClientException, NotAuthorizedException
from litestar.testing import TestClient
from sqlalchemy import func, select

from buildapp.bridge_http_controller import BridgeHttpController, fields
from buildapp.db_test_support import asgi_app, in_memory_session_maker
from buildapp.models import BridgeHttpChallenge, BridgeHttpReplay
from buildapp.test_bridge_http_auth import ISSUER, paired, signed


def test_unsigned_registration_requests_never_allocate_device_budget(monkeypatch):
    monkeypatch.setattr(
        BridgeHttpController,
        "dependencies",
        {"public_base_url": Provide(lambda: ISSUER, sync_to_thread=False)},
    )
    app = asgi_app([BridgeHttpController], session_maker=in_memory_session_maker())
    with TestClient(app=app) as client:

        async def seed():
            async with app.state.make_session() as db:
                _, device, key = await paired(db)
                return device, key

        with client.portal() as portal:
            device, key = portal.call(seed)
        device_id = device.id
        route = f"/api/devices/{device_id}/http-endpoint/challenge"
        for _ in range(10):
            result = client.post(
                f"/api/devices/{device_id}/http-endpoint/challenge", json={}
            )
            assert result.status_code in (400, 401), result.text

        async def counts():
            async with app.state.make_session() as db:
                return tuple(
                    [
                        await db.scalar(select(func.count()).select_from(model))
                        for model in (BridgeHttpChallenge, BridgeHttpReplay)
                    ]
                )

        with client.portal() as portal:
            assert portal.call(counts) == (0, 0)

        for body in (
            {"proof": None},
            {"proof": True},
            {"proof": []},
            {"proof": "x" * 16001},
        ):
            assert client.post(route, json=body).status_code == 401
        for raw in [
            b'{"proof":',
            b'{"proof":' + b"[" * 1200 + b"0" + b"]" * 1200 + b"}",
        ]:
            assert client.post(
                route, content=raw, headers={"content-type": "application/json"}
            ).status_code in (400, 401)
        assert client.post(route, json={"proof": "x" * 18001}).status_code == 413
        with client.portal() as portal:
            assert portal.call(counts) == (0, 0)
        # Failed unauthenticated attempts leave capacity for the actual device.
        assert (
            client.post(
                route, json={"proof": signed(key, device, "registration-challenge")}
            ).status_code
            == 201
        )
        with client.portal() as portal:
            assert portal.call(counts) == (1, 1)


@pytest.mark.asyncio
async def test_body_limit_is_checked_before_accumulating_untrusted_chunk():
    class LargeChunk:
        def __len__(self):
            return 18001

        def __iter__(self):
            raise AssertionError("oversized chunk was copied")

    class Request:
        content_type = ("application/json", {})

        async def stream(self):
            yield LargeChunk()

    with pytest.raises(ClientException) as error:
        await fields(Request(), ("proof",), 18000)
    assert error.value.status_code == 413


@pytest.mark.asyncio
async def test_valid_proof_is_spent_once_and_only_allocates_nonce(db):
    from buildapp import bridge_http_auth as auth
    from buildapp.clock import utc_now
    from buildapp.test_bridge_http_auth import signed

    _, device, key = await paired(db)
    proof = signed(key, device, "registration-challenge")
    first = await auth.registration_challenge(db, device.id, proof, ISSUER, utc_now())
    await db.commit()
    assert first["nonce"] and first["challenge_id"]
    with pytest.raises(NotAuthorizedException) as error:
        await auth.registration_challenge(db, device.id, proof, ISSUER, utc_now())
    assert error.value.status_code == 401
    await db.commit()
    assert await db.scalar(select(func.count()).select_from(BridgeHttpChallenge)) == 1
    assert await db.scalar(select(func.count()).select_from(BridgeHttpReplay)) == 1
    from buildapp.models import BridgeHttpEndpoint, BridgeHttpFamily

    assert await db.scalar(select(func.count()).select_from(BridgeHttpEndpoint)) == 0
    assert await db.scalar(select(func.count()).select_from(BridgeHttpFamily)) == 0


@pytest.mark.asyncio
async def test_capacity_failure_rolls_back_proof_spend_and_can_retry_uncommitted_proof(
    db,
):
    from datetime import timedelta
    from buildapp import bridge_http_auth as auth
    from buildapp.clock import utc_now
    from buildapp.test_bridge_http_auth import signed
    from litestar.exceptions import TooManyRequestsException

    _, device, key = await paired(db)
    now = utc_now()
    for _ in range(8):
        await auth.registration_challenge(
            db, device.id, signed(key, device, "registration-challenge"), ISSUER, now
        )
        await db.commit()
    ninth = signed(key, device, "registration-challenge")
    with pytest.raises(TooManyRequestsException):
        await auth.registration_challenge(db, device.id, ninth, ISSUER, now)
    await (
        db.commit()
    )  # Even a caller committing after caught failure has no partial JTI.
    assert await db.scalar(select(func.count()).select_from(BridgeHttpReplay)) == 8
    assert await db.scalar(select(func.count()).select_from(BridgeHttpChallenge)) == 8
    oldest = await db.scalar(select(BridgeHttpChallenge).limit(1))
    oldest.expires_at = now - timedelta(seconds=1)
    await db.commit()
    await auth.registration_challenge(db, device.id, ninth, ISSUER, now)
    await db.commit()
    assert await db.scalar(select(func.count()).select_from(BridgeHttpChallenge)) == 8
    assert await db.scalar(select(func.count()).select_from(BridgeHttpReplay)) == 9


@pytest.mark.asyncio
async def test_nonce_write_failure_rolls_back_jti_and_nonce_together(db, monkeypatch):
    from buildapp import bridge_http_auth as auth
    from buildapp.clock import utc_now
    from buildapp.test_bridge_http_auth import signed

    _, device, key = await paired(db)
    proof = signed(key, device, "registration-challenge")
    original = auth._challenge

    async def broken(*args, **kwargs):
        await original(
            *args, **kwargs
        )  # Failure after the nonce INSERT also rolls back.
        raise RuntimeError("injected storage failure")

    with monkeypatch.context() as patch:
        patch.setattr(auth, "_challenge", broken)
        with pytest.raises(RuntimeError, match="injected storage failure"):
            await auth.registration_challenge(db, device.id, proof, ISSUER, utc_now())
    await db.commit()
    assert await db.scalar(select(func.count()).select_from(BridgeHttpChallenge)) == 0
    assert await db.scalar(select(func.count()).select_from(BridgeHttpReplay)) == 0
    await auth.registration_challenge(db, device.id, proof, ISSUER, utc_now())
    await db.commit()
    assert await db.scalar(select(func.count()).select_from(BridgeHttpChallenge)) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "change",
    [
        "aud",
        "purpose",
        "device",
        "key",
        "iat",
        "exp",
        "bool-time",
        "float-time",
        "jti",
        "oversized",
    ],
)
async def test_invalid_allocation_proofs_create_no_nonce_or_replay(db, change):
    import jwt
    from uuid import uuid4
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    from litestar.exceptions import NotAuthorizedException
    from buildapp import bridge_http_auth as auth
    from buildapp.clock import utc_now

    _, device, key = await paired(db)
    timestamp = int(utc_now().timestamp())
    claims = {
        "iss": str(device.id),
        "sub": str(device.id),
        "aud": auth.audience(ISSUER, "registration-challenge", str(device.id)),
        "iat": timestamp,
        "exp": timestamp + 30,
        "jti": str(uuid4()),
    }
    other_device = str(uuid4())
    updates = {
        "aud": {"aud": ISSUER + "/api/bridge-http/introspect"},
        "device": {"iss": other_device, "sub": other_device},
        "iat": {"iat": timestamp + 6, "exp": timestamp + 36},
        "exp": {"iat": timestamp - 40, "exp": timestamp - 6},
        "bool-time": {"iat": True},
        "float-time": {"exp": float(timestamp + 30)},
        "jti": {"jti": "x"},
        "oversized": {"extra": "x" * 16000},
    }
    claims.update(updates.get(change, {}))
    purpose = "registration" if change == "purpose" else "registration-challenge"
    key = Ed25519PrivateKey.generate() if change == "key" else key
    proof = jwt.encode(
        claims, key, algorithm="EdDSA", headers={"typ": auth.TYPES[purpose]}
    )
    with pytest.raises(NotAuthorizedException):
        await auth.registration_challenge(db, device.id, proof, ISSUER, utc_now())
    assert await db.scalar(select(func.count()).select_from(BridgeHttpChallenge)) == 0
    assert await db.scalar(select(func.count()).select_from(BridgeHttpReplay)) == 0


@pytest.mark.asyncio
async def test_allocation_skew_and_replay_retention_end_at_exp_plus_five(db):
    import jwt
    from datetime import datetime, timezone
    from uuid import uuid4
    from litestar.exceptions import NotAuthorizedException
    from buildapp import bridge_http_auth as auth

    _, device, key = await paired(db)
    now = 1800000000
    claims = {
        "iss": str(device.id),
        "sub": str(device.id),
        "aud": auth.audience(ISSUER, "registration-challenge", str(device.id)),
        "iat": now,
        "exp": now + 30,
        "jti": str(uuid4()),
    }
    proof = jwt.encode(
        claims,
        key,
        algorithm="EdDSA",
        headers={"typ": auth.TYPES["registration-challenge"]},
    )

    def at(offset):
        return datetime.fromtimestamp(now + offset, timezone.utc)

    await auth.registration_challenge(db, device.id, proof, ISSUER, at(35))
    await db.commit()
    record = await db.scalar(select(BridgeHttpReplay))
    assert record.expires_at == at(35)
    with pytest.raises(NotAuthorizedException):
        await auth.registration_challenge(db, device.id, proof, ISSUER, at(35))
    with pytest.raises(NotAuthorizedException):
        await auth.registration_challenge(db, device.id, proof, ISSUER, at(36))
    assert await db.scalar(select(func.count()).select_from(BridgeHttpChallenge)) == 1


@pytest.mark.asyncio
async def test_allocation_replay_capacity_refuses_without_evicting_live_guards(db):
    from datetime import timedelta
    from uuid import uuid4
    from litestar.exceptions import TooManyRequestsException
    from buildapp import bridge_http_auth as auth
    from buildapp.clock import utc_now

    _, device, key = await paired(db)
    now = utc_now()
    db.add_all(
        [
            BridgeHttpReplay(
                device_id=device.id,
                jti=str(uuid4()),
                expires_at=now + timedelta(seconds=35),
            )
            for _ in range(1024)
        ]
    )
    await db.commit()
    with pytest.raises(TooManyRequestsException):
        await auth.registration_challenge(
            db, device.id, signed(key, device, "registration-challenge"), ISSUER, now
        )
    await db.commit()
    assert await db.scalar(select(func.count()).select_from(BridgeHttpReplay)) == 1024
    assert await db.scalar(select(func.count()).select_from(BridgeHttpChallenge)) == 0


@pytest.mark.asyncio
async def test_legacy_pending_capacity_recovers_after_last_allocation_expires(db):
    import jwt
    from datetime import datetime, timedelta, timezone
    from uuid import uuid4
    from litestar.exceptions import TooManyRequestsException
    from buildapp import bridge_http_auth as auth

    _, device, key = await paired(db)
    now = datetime.fromtimestamp(1800000000, timezone.utc)
    # Simulates the last old unsigned worker allocations, not new authorized ones.
    for _ in range(8):
        await auth._challenge(db, now, purpose="registration", device_id=device.id)
    await db.commit()

    def proof(at):
        timestamp = int(at.timestamp())
        return jwt.encode(
            {
                "iss": str(device.id),
                "sub": str(device.id),
                "aud": auth.audience(ISSUER, "registration-challenge", str(device.id)),
                "iat": timestamp,
                "exp": timestamp + 30,
                "jti": str(uuid4()),
            },
            key,
            algorithm="EdDSA",
            headers={"typ": auth.TYPES["registration-challenge"]},
        )

    with pytest.raises(TooManyRequestsException):
        await auth.registration_challenge(db, device.id, proof(now), ISSUER, now)
    await db.commit()
    later = now + timedelta(seconds=61)
    await auth.registration_challenge(db, device.id, proof(later), ISSUER, later)
    await db.commit()
    assert await db.scalar(select(func.count()).select_from(BridgeHttpChallenge)) == 1
    assert await db.scalar(select(func.count()).select_from(BridgeHttpReplay)) == 1
