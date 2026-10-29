"""Account and endpoint authority is established from paired keys, never URL input."""

import base64
from datetime import datetime, timedelta, timezone
from uuid import UUID, uuid4

import jwt
import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
from litestar.exceptions import NotAuthorizedException

from buildapp import bridge_http_auth as auth
from buildapp.clock import utc_now
from buildapp.db_test_support import add_account, add_member
from buildapp.models import Device

ISSUER = "https://app.example"
ORIGIN = "https://bridge.example:8443"


async def paired(db):
    user = await add_account(db, "owner@example.com")
    await add_member(db, user, "owner@example.com")
    key = Ed25519PrivateKey.generate()
    device = Device(
        name="Laptop",
        owner_user_id=user,
        approved=True,
        identity_public_key_b64=base64.b64encode(
            key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
        ).decode(),
        transport_public_key_b64="unused",
    )
    db.add(device)
    await db.commit()
    return user, device, key


def signed(key, device, purpose, **fields):
    now = int(utc_now().timestamp())
    audience = auth.audience(ISSUER, purpose, str(device.id))
    claims = dict(
        iss=str(device.id),
        sub=str(device.id),
        aud=audience,
        iat=now,
        exp=now + 30,
        jti=str(uuid4()),
        **fields,
    )
    return jwt.encode(
        claims, key, algorithm="EdDSA", headers={"typ": auth.TYPES[purpose]}
    )


async def registered(db):
    user, device, key = await paired(db)
    challenge = await auth.registration_challenge(db, device.id, utc_now())
    proof = signed(
        key,
        device,
        "registration",
        challenge_id=challenge["challenge_id"],
        nonce=challenge["nonce"],
        origin=ORIGIN,
        endpoint_epoch="epoch-one",
        previous_endpoint_epoch=None,
        enabled=True,
    )
    await auth.register_endpoint(db, device.id, proof, ISSUER, utc_now())
    await db.commit()
    return user, device, key


@pytest.mark.asyncio
async def test_discovery_mints_targeted_token_and_refresh_keeps_family(db):
    user, device, key = await registered(db)
    challenge = await auth.create_discovery(
        db, user, "session-one", ORIGIN, ISSUER, utc_now()
    )
    proof = signed(
        key,
        device,
        "discovery",
        challenge_id=challenge["challenge_id"],
        nonce=challenge["nonce"],
        origin=ORIGIN,
        endpoint_epoch="epoch-one",
        issuer=ISSUER,
    )
    minted = await auth.complete_discovery(
        db, user, "session-one", challenge["challenge_id"], proof, ISSUER, utc_now()
    )
    assertion = signed(
        key, device, "introspection", token_hash=auth.hash_token(minted["access_token"])
    )
    result = await auth.introspect(
        db, minted["access_token"], assertion, ISSUER, utc_now()
    )
    assert result["active"] is True
    assert result["account_id"] == str(user)
    assert result["origin"] == ORIGIN
    refreshed = await auth.renew(
        db,
        user,
        "session-one",
        device.id,
        minted["family_id"],
        "epoch-one",
        ISSUER,
        utc_now(),
    )
    assert refreshed["family_id"] == minted["family_id"]
    assert refreshed["access_token"] != minted["access_token"]
    with pytest.raises(NotAuthorizedException):
        await auth.renew(
            db,
            user,
            "another-session",
            device.id,
            minted["family_id"],
            "epoch-one",
            ISSUER,
            utc_now(),
        )


@pytest.mark.asyncio
async def test_forwarded_proof_cross_account_and_replays_are_refused(db):
    user, device, key = await registered(db)
    challenge = await auth.create_discovery(
        db, user, "s", "https://evil.example", ISSUER, utc_now()
    )
    proof = signed(
        key,
        device,
        "discovery",
        challenge_id=challenge["challenge_id"],
        nonce=challenge["nonce"],
        origin=ORIGIN,
        endpoint_epoch="epoch-one",
        issuer=ISSUER,
    )
    with pytest.raises(NotAuthorizedException):
        await auth.complete_discovery(
            db, user, "s", challenge["challenge_id"], proof, ISSUER, utc_now()
        )
    challenge = await auth.create_discovery(db, user, "s", ORIGIN, ISSUER, utc_now())
    proof = signed(
        key,
        device,
        "discovery",
        challenge_id=challenge["challenge_id"],
        nonce=challenge["nonce"],
        origin=ORIGIN,
        endpoint_epoch="epoch-one",
        issuer=ISSUER,
    )
    with pytest.raises(NotAuthorizedException):
        await auth.complete_discovery(
            db, uuid4(), "s", challenge["challenge_id"], proof, ISSUER, utc_now()
        )
    await auth.complete_discovery(
        db, user, "s", challenge["challenge_id"], proof, ISSUER, utc_now()
    )
    with pytest.raises(NotAuthorizedException):
        await auth.complete_discovery(
            db, user, "s", challenge["challenge_id"], proof, ISSUER, utc_now()
        )


@pytest.mark.parametrize(
    "value",
    [
        "http://host",
        "https://user@host",
        "https://host/a",
        "https://host?x",
        "https://host.",
        "https://127.1",
        "https://[fe80::1%25eth0]",
        "https://host\r\nX: y",
    ],
)
def test_origin_rejects_ambiguous_or_header_input(value):
    with pytest.raises(ValueError):
        auth.canonical_origin(value)


def test_origin_canonicalizes_without_fetching():
    assert auth.canonical_origin("https://EXAMPLE.com:443/") == "https://example.com"
    assert auth.canonical_origin("https://[::1]:8443") == "https://[::1]:8443"


@pytest.mark.asyncio
async def test_logout_endpoint_change_and_membership_revoke_invalidate_tokens(db):
    user, device, key = await registered(db)
    challenge = await auth.create_discovery(db, user, "s", ORIGIN, ISSUER, utc_now())
    proof = signed(
        key,
        device,
        "discovery",
        challenge_id=challenge["challenge_id"],
        nonce=challenge["nonce"],
        origin=ORIGIN,
        endpoint_epoch="epoch-one",
        issuer=ISSUER,
    )
    minted = await auth.complete_discovery(
        db, user, "s", challenge["challenge_id"], proof, ISSUER, utc_now()
    )
    await auth.revoke_client(db, user, "s", utc_now())
    assertion = signed(
        key, device, "introspection", token_hash=auth.hash_token(minted["access_token"])
    )
    assert await auth.introspect(
        db, minted["access_token"], assertion, ISSUER, utc_now()
    ) == {"active": False}
    with pytest.raises(NotAuthorizedException):
        await auth.renew(
            db,
            user,
            "s",
            device.id,
            minted["family_id"],
            "epoch-one",
            ISSUER,
            utc_now() + timedelta(minutes=6),
        )


@pytest.mark.asyncio
async def test_logged_out_binding_cannot_start_fresh_discovery(db):
    user, _, _ = await registered(db)
    await auth.revoke_client(db, user, "logged-out-session", utc_now())
    with pytest.raises(NotAuthorizedException):
        await auth.create_discovery(
            db, user, "logged-out-session", ORIGIN, ISSUER, utc_now()
        )


@pytest.mark.asyncio
async def test_introspection_assertion_replay_cannot_be_reused(db):
    user, device, key = await registered(db)
    pending = await auth.create_discovery(db, user, "s", ORIGIN, ISSUER, utc_now())
    proof = signed(
        key,
        device,
        "discovery",
        challenge_id=pending["challenge_id"],
        nonce=pending["nonce"],
        origin=ORIGIN,
        endpoint_epoch="epoch-one",
        issuer=ISSUER,
    )
    token = (
        await auth.complete_discovery(
            db, user, "s", pending["challenge_id"], proof, ISSUER, utc_now()
        )
    )["access_token"]
    assertion = signed(key, device, "introspection", token_hash=auth.hash_token(token))
    assert (await auth.introspect(db, token, assertion, ISSUER, utc_now()))["active"]
    with pytest.raises(NotAuthorizedException):
        await auth.introspect(db, token, assertion, ISSUER, utc_now())


@pytest.mark.asyncio
async def test_desktop_parent_expiry_and_revocation_cap_the_family(db):
    from skrift.db.services import oauth2_service

    user, device, key = await registered(db)
    parent_exp = int(utc_now().timestamp()) + 60
    parent_jti = "a" * 32
    client = f"desktop:{parent_jti}:{parent_exp}"
    pending = await auth.create_discovery(db, user, client, ORIGIN, ISSUER, utc_now())
    proof = signed(
        key,
        device,
        "discovery",
        challenge_id=pending["challenge_id"],
        nonce=pending["nonce"],
        origin=ORIGIN,
        endpoint_epoch="epoch-one",
        issuer=ISSUER,
    )
    minted = await auth.complete_discovery(
        db, user, client, pending["challenge_id"], proof, ISSUER, utc_now()
    )
    assert minted["expires_at"] == parent_exp
    await oauth2_service.revoke_token(
        db, parent_jti, "access", datetime.fromtimestamp(parent_exp, timezone.utc)
    )
    assertion = signed(
        key, device, "introspection", token_hash=auth.hash_token(minted["access_token"])
    )
    assert await auth.introspect(
        db, minted["access_token"], assertion, ISSUER, utc_now()
    ) == {"active": False}
    with pytest.raises(NotAuthorizedException):
        await auth.renew(
            db,
            user,
            client,
            device.id,
            minted["family_id"],
            "epoch-one",
            ISSUER,
            utc_now(),
        )


def test_idna_origin_and_empty_port_are_unambiguous():
    assert auth.canonical_origin("https://faß.de") == "https://xn--fa-hia.de"
    with pytest.raises(ValueError):
        auth.canonical_origin("https://example.com:")


@pytest.mark.asyncio
async def test_suspended_account_cannot_mint_or_verify_endpoint(db):
    from skrift.db.models.user import User

    user, device, _ = await registered(db)
    (await db.get(User, user)).is_active = False
    await db.flush()
    with pytest.raises(NotAuthorizedException):
        await auth.create_discovery(db, user, "s", ORIGIN, ISSUER, utc_now())
    with pytest.raises(NotAuthorizedException):
        await auth.registration_challenge(db, device.id, utc_now())


@pytest.mark.asyncio
async def test_endpoint_change_and_alpha_revoke_cannot_renew_old_family(db):
    from buildapp.models import BridgeHttpFamily, Invite
    from sqlalchemy import select

    user, device, key = await registered(db)
    pending = await auth.create_discovery(db, user, "s", ORIGIN, ISSUER, utc_now())
    proof = signed(
        key,
        device,
        "discovery",
        challenge_id=pending["challenge_id"],
        nonce=pending["nonce"],
        origin=ORIGIN,
        endpoint_epoch="epoch-one",
        issuer=ISSUER,
    )
    minted = await auth.complete_discovery(
        db, user, "s", pending["challenge_id"], proof, ISSUER, utc_now()
    )
    challenge = await auth.registration_challenge(db, device.id, utc_now())
    proof = signed(
        key,
        device,
        "registration",
        challenge_id=challenge["challenge_id"],
        nonce=challenge["nonce"],
        origin="https://new.example",
        endpoint_epoch="epoch-two",
        previous_endpoint_epoch="epoch-one",
        enabled=True,
    )
    await auth.register_endpoint(db, device.id, proof, ISSUER, utc_now())
    assert (
        await db.get(BridgeHttpFamily, UUID(minted["family_id"]))
    ).revoked_at is not None
    with pytest.raises(NotAuthorizedException):
        await auth.renew(
            db,
            user,
            "s",
            device.id,
            minted["family_id"],
            "epoch-one",
            ISSUER,
            utc_now(),
        )
    invite = await db.scalar(select(Invite).where(Invite.redeemed_by == user))
    invite.revoked_at = utc_now()
    await db.flush()
    with pytest.raises(NotAuthorizedException):
        await auth.create_discovery(
            db, user, "fresh", "https://new.example", ISSUER, utc_now()
        )


@pytest.mark.asyncio
async def test_other_owned_device_cannot_introspect_target_token(db):
    user, device, key = await registered(db)
    pending = await auth.create_discovery(db, user, "s", ORIGIN, ISSUER, utc_now())
    proof = signed(
        key,
        device,
        "discovery",
        challenge_id=pending["challenge_id"],
        nonce=pending["nonce"],
        origin=ORIGIN,
        endpoint_epoch="epoch-one",
        issuer=ISSUER,
    )
    minted = await auth.complete_discovery(
        db, user, "s", pending["challenge_id"], proof, ISSUER, utc_now()
    )
    other_key = Ed25519PrivateKey.generate()
    other = Device(
        name="Other",
        approved=True,
        owner_user_id=user,
        identity_public_key_b64=base64.b64encode(
            other_key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
        ).decode(),
        transport_public_key_b64="unused",
    )
    db.add(other)
    await db.flush()
    assertion = signed(
        other_key,
        other,
        "introspection",
        token_hash=auth.hash_token(minted["access_token"]),
    )
    assert await auth.introspect(
        db, minted["access_token"], assertion, ISSUER, utc_now()
    ) == {"active": False}


@pytest.mark.asyncio
async def test_replay_budget_refuses_instead_of_evicting_live_guards(db):
    from buildapp.models import BridgeHttpReplay
    from litestar.exceptions import TooManyRequestsException

    _, device, key = await registered(db)
    db.add_all(
        [
            BridgeHttpReplay(
                device_id=device.id,
                jti=str(uuid4()),
                expires_at=utc_now() + timedelta(seconds=35),
            )
            for _ in range(1024)
        ]
    )
    await db.flush()
    raw = "bhttp_" + "x" * 43
    assertion = signed(key, device, "introspection", token_hash=auth.hash_token(raw))
    with pytest.raises(TooManyRequestsException):
        await auth.introspect(db, raw, assertion, ISSUER, utc_now())


@pytest.mark.parametrize(
    "value", ["https://[::1]suffix", "https://[::1]:", "https://１２７.０.０.１"]
)
def test_origin_rejects_malformed_authority_and_mapped_ip_aliases(value):
    with pytest.raises(ValueError):
        auth.canonical_origin(value)


async def mint_for_client(db, user, device, key, binding):
    pending = await auth.create_discovery(db, user, binding, ORIGIN, ISSUER, utc_now())
    proof = signed(
        key,
        device,
        "discovery",
        challenge_id=pending["challenge_id"],
        nonce=pending["nonce"],
        origin=ORIGIN,
        endpoint_epoch="epoch-one",
        issuer=ISSUER,
    )
    return await auth.complete_discovery(
        db, user, binding, pending["challenge_id"], proof, ISSUER, utc_now()
    )


@pytest.mark.asyncio
async def test_two_same_account_desktop_issuances_isolate_logout_and_rotation(db):
    user, device, key = await registered(db)
    exp = int(utc_now().timestamp()) + 600
    first_binding, second_binding = (
        f"desktop:{'a' * 32}:{exp}",
        f"desktop:{'b' * 32}:{exp}",
    )
    first = await mint_for_client(db, user, device, key, first_binding)
    second = await mint_for_client(db, user, device, key, second_binding)
    browser = await mint_for_client(db, user, device, key, "browser:session")
    with pytest.raises(NotAuthorizedException):
        await auth.renew(
            db,
            user,
            second_binding,
            device.id,
            first["family_id"],
            "epoch-one",
            ISSUER,
            utc_now(),
        )
    await auth.revoke_client(db, user, first_binding, utc_now())
    for minted, expected in ((first, False), (second, True), (browser, True)):
        assertion = signed(
            key,
            device,
            "introspection",
            token_hash=auth.hash_token(minted["access_token"]),
        )
        assert (
            await auth.introspect(
                db, minted["access_token"], assertion, ISSUER, utc_now()
            )
        )["active"] is expected
    renewed = await auth.renew(
        db,
        user,
        second_binding,
        device.id,
        second["family_id"],
        "epoch-one",
        ISSUER,
        utc_now(),
    )
    assert renewed["family_id"] == second["family_id"]
    assert browser["expires_at"] >= int(utc_now().timestamp()) + 298


@pytest.mark.asyncio
async def test_desktop_near_expiry_parent_has_no_authorization_grace(db):
    user, device, key = await registered(db)
    exp = int(utc_now().timestamp()) + 2
    binding = f"desktop:{'c' * 32}:{exp}"
    minted = await mint_for_client(db, user, device, key, binding)
    assert minted["expires_at"] == exp
    exact_deadline = datetime.fromtimestamp(exp, timezone.utc)
    with pytest.raises(NotAuthorizedException):
        await auth.renew(
            db,
            user,
            binding,
            device.id,
            minted["family_id"],
            "epoch-one",
            ISSUER,
            exact_deadline,
        )
    assertion = signed(
        key, device, "introspection", token_hash=auth.hash_token(minted["access_token"])
    )
    assert await auth.introspect(
        db, minted["access_token"], assertion, ISSUER, exact_deadline
    ) == {"active": False}


@pytest.mark.asyncio
async def test_changed_paired_identity_key_invalidates_old_endpoint(db):
    user, device, key = await registered(db)
    new_key = Ed25519PrivateKey.generate()
    device.identity_public_key_b64 = base64.b64encode(
        new_key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
    ).decode()
    await db.flush()
    with pytest.raises(NotAuthorizedException):
        await mint_for_client(db, user, device, new_key, "browser:changed-key")
