"""Hosted authority transactions for opt-in account-bound bridge HTTP.

Callers commit only after a whole operation succeeds. Conditional challenge spends
and unique replay records fence competing workers; row locks serialize epoch changes.
"""

import hmac
import secrets
from datetime import datetime, timedelta, timezone
from uuid import UUID

from litestar.exceptions import NotAuthorizedException, TooManyRequestsException
from sqlalchemy import delete, func, select, update
from sqlalchemy.exc import IntegrityError
from skrift.db.models.user import User
from skrift.db.services import oauth2_service

from buildapp.alpha_membership import is_alpha_member
from buildapp.bridge_http_models import (
    BridgeHttpClient,
    BridgeHttpChallenge,
    BridgeHttpEndpoint,
    BridgeHttpFamily,
    BridgeHttpReplay,
    BridgeHttpToken,
)
from buildapp.bridge_http_proofs import (
    TYPES as TYPES,
    audience as audience,
    canonical_origin,
    proof_device_id,
    verify,
)
from buildapp.models import Device
from buildapp.pairing_crypto import fingerprint
from buildapp.token_hash import token_hash as hash_token

SCOPE = "bridge:read bridge:write"


def refuse():
    raise NotAuthorizedException("Bridge authority refused")


async def current_device(db, device_id):
    device = await db.scalar(
        select(Device).where(Device.id == device_id).with_for_update()
    )
    if device is None or not device.approved or device.owner_user_id is None:
        refuse()
    if not await _active_member(db, device.owner_user_id):
        refuse()
    return device


async def registration_challenge(db, device_id, now):
    await current_device(db, device_id)
    await _bounded_challenges(db, BridgeHttpChallenge.device_id == device_id, now, 8)
    challenge, nonce = await _challenge(
        db, now, purpose="registration", device_id=device_id
    )
    return {
        "challenge_id": str(challenge.id),
        "nonce": nonce,
        "expires_at": int(challenge.expires_at.timestamp()),
    }


async def _bounded_challenges(db, owner, now, maximum):
    await db.execute(
        delete(BridgeHttpChallenge).where(BridgeHttpChallenge.expires_at < now)
    )
    count = await db.scalar(
        select(func.count())
        .select_from(BridgeHttpChallenge)
        .where(owner, BridgeHttpChallenge.consumed_at.is_(None))
    )
    if count >= maximum:
        raise TooManyRequestsException("Too many pending challenges")


async def _challenge(db, now, **fields):
    nonce = secrets.token_urlsafe(32)
    challenge = BridgeHttpChallenge(
        **fields,
        nonce=nonce,
        nonce_hash=hash_token(nonce),
        expires_at=now + timedelta(seconds=60),
    )
    db.add(challenge)
    await db.flush()
    return challenge, nonce


async def _spend(db, challenge, nonce, now):
    if not isinstance(nonce, str) or not hmac.compare_digest(
        challenge.nonce_hash, hash_token(nonce)
    ):
        refuse()
    result = await db.execute(
        update(BridgeHttpChallenge)
        .where(
            BridgeHttpChallenge.id == challenge.id,
            BridgeHttpChallenge.consumed_at.is_(None),
            BridgeHttpChallenge.expires_at > now,
        )
        .values(consumed_at=now)
    )
    if result.rowcount != 1:
        refuse()


async def _record_replay(db, device, claims, now):
    await db.execute(delete(BridgeHttpReplay).where(BridgeHttpReplay.expires_at < now))
    count = await db.scalar(
        select(func.count())
        .select_from(BridgeHttpReplay)
        .where(BridgeHttpReplay.device_id == device.id)
    )
    if count >= 1024:
        raise TooManyRequestsException("Device replay budget exhausted")
    row = BridgeHttpReplay(
        device_id=device.id,
        jti=claims["jti"],
        expires_at=datetime.fromtimestamp(claims["exp"] + 5, timezone.utc),
    )
    try:
        async with db.begin_nested():
            db.add(row)
            await db.flush()
    except IntegrityError:
        refuse()


async def register_endpoint(db, device_id, proof, issuer, now):
    device = await current_device(db, device_id)
    claims = verify(proof, device, "registration", issuer, now)
    challenge = await _get_challenge(db, claims.get("challenge_id"), "registration")
    if challenge.device_id != device.id:
        refuse()
    endpoint = await _endpoint(db, device.id)
    _validate_registration(claims, endpoint, device)
    await _record_replay(db, device, claims, now)
    await _spend(db, challenge, claims.get("nonce"), now)
    changed = endpoint is not None and (
        endpoint.origin,
        endpoint.endpoint_epoch,
        endpoint.enabled,
    ) != (claims["origin"], claims["endpoint_epoch"], claims["enabled"])
    if changed:
        await revoke_device(db, device.id, now, bump_epoch=False)
    if endpoint is None:
        endpoint = BridgeHttpEndpoint(device_id=device.id)
        db.add(endpoint)
    endpoint.origin = claims["origin"]
    endpoint.endpoint_epoch = claims["endpoint_epoch"]
    endpoint.enabled = claims["enabled"]
    endpoint.user_id = device.owner_user_id
    endpoint.authorization_epoch = device.authorization_epoch
    endpoint.identity_fingerprint = fingerprint(device.identity_public_key_b64)
    await db.flush()
    return {
        "origin": endpoint.origin,
        "endpoint_epoch": endpoint.endpoint_epoch,
        "enabled": endpoint.enabled,
    }


def _validate_registration(claims, endpoint, device):
    try:
        origin = canonical_origin(claims["origin"])
        epoch = claims["endpoint_epoch"]
        if origin != claims["origin"] or type(claims["enabled"]) is not bool:
            refuse()
        if not isinstance(epoch, str) or not 1 <= len(epoch) <= 128:
            refuse()
        previous = claims.get("previous_endpoint_epoch")
        if previous != (endpoint.endpoint_epoch if endpoint else None):
            refuse()
        if (
            endpoint
            and epoch == endpoint.endpoint_epoch
            and (
                (origin, claims["enabled"]) != (endpoint.origin, endpoint.enabled)
                or endpoint.identity_fingerprint
                != fingerprint(device.identity_public_key_b64)
            )
        ):
            refuse()
    except (ValueError, KeyError):
        refuse()


async def _get_challenge(db, challenge_id, purpose):
    try:
        challenge = await db.get(BridgeHttpChallenge, UUID(str(challenge_id)))
    except ValueError:
        refuse()
    if challenge is None or challenge.purpose != purpose:
        refuse()
    return challenge


async def _endpoint(db, device_id):
    return await db.scalar(
        select(BridgeHttpEndpoint).where(BridgeHttpEndpoint.device_id == device_id)
    )


async def create_discovery(db, user_id, client_binding, origin, issuer, now):
    # Serializes bounded per-account challenge allocation across hosted workers.
    await db.scalar(select(User).where(User.id == user_id).with_for_update())
    if not await _active_member(db, user_id):
        refuse()
    await _client_active(db, user_id, client_binding, now)
    origin = canonical_origin(origin)
    await _bounded_challenges(db, BridgeHttpChallenge.user_id == user_id, now, 8)
    challenge, _ = await _challenge(
        db,
        now,
        purpose="discovery",
        user_id=user_id,
        client_binding=client_binding,
        origin=origin,
    )
    return discovery_payload(challenge, issuer)


def discovery_payload(challenge, issuer):
    return {
        "challenge_id": str(challenge.id),
        "nonce": challenge.nonce,
        "origin": challenge.origin,
        "expires_at": int(challenge.expires_at.timestamp()),
        "issuer": canonical_origin(issuer),
        "account_id": str(challenge.user_id),
        "resume_url": f"/app/?bridge_discovery={challenge.id}",
    }


async def pending_discovery(db, user_id, client_binding, challenge_id, issuer, now):
    await _client_active(db, user_id, client_binding, now)
    challenge = await _get_challenge(db, challenge_id, "discovery")
    _check_pending(challenge, user_id, client_binding, now)
    return discovery_payload(challenge, issuer)


def _check_pending(challenge, user_id, client_binding, now):
    if (challenge.user_id, challenge.client_binding) != (user_id, client_binding):
        refuse()
    if challenge.expires_at <= now or challenge.consumed_at is not None:
        refuse()


async def complete_discovery(
    db, user_id, client_binding, challenge_id, proof, issuer, now
):
    await _client_active(db, user_id, client_binding, now)
    device = await current_device(db, proof_device_id(proof))
    if device.owner_user_id != user_id:
        refuse()
    claims = verify(proof, device, "discovery", issuer, now)
    challenge = await _get_challenge(db, challenge_id, "discovery")
    _check_pending(challenge, user_id, client_binding, now)
    endpoint = await _endpoint(db, device.id)
    if not _valid_endpoint(endpoint, device):
        refuse()
    expected = (
        str(challenge.id),
        challenge.origin,
        endpoint.endpoint_epoch,
        canonical_origin(issuer),
    )
    actual = tuple(
        claims.get(key)
        for key in ("challenge_id", "origin", "endpoint_epoch", "issuer")
    )
    if actual != expected or claims["origin"] != endpoint.origin:
        refuse()
    await _record_replay(db, device, claims, now)
    await _spend(db, challenge, claims.get("nonce"), now)
    family = BridgeHttpFamily(
        issuer=canonical_origin(issuer),
        user_id=user_id,
        device_id=device.id,
        client_binding=client_binding,
        origin=endpoint.origin,
        endpoint_epoch=endpoint.endpoint_epoch,
        authorization_epoch=device.authorization_epoch,
        scope=SCOPE,
        **_parent_fields(client_binding),
    )
    db.add(family)
    await db.flush()
    return await _mint(db, family, device, now)


def _valid_endpoint(endpoint, device):
    return (
        endpoint is not None
        and endpoint.enabled
        and endpoint.user_id == device.owner_user_id
        and endpoint.authorization_epoch == device.authorization_epoch
        and endpoint.identity_fingerprint == fingerprint(device.identity_public_key_b64)
    )


async def _mint(db, family, device, now):
    await db.execute(delete(BridgeHttpToken).where(BridgeHttpToken.expires_at < now))
    raw = "bhttp_" + secrets.token_urlsafe(32)
    token = BridgeHttpToken(
        family_id=family.id,
        token_hash=hash_token(raw),
        expires_at=min(now + timedelta(minutes=5), family.parent_expires_at)
        if family.parent_expires_at
        else now + timedelta(minutes=5),
    )
    db.add(token)
    await db.flush()
    return {
        "access_token": raw,
        "expires_at": int(token.expires_at.timestamp()),
        "family_id": str(family.id),
        "account_id": str(family.user_id),
        "device_id": str(device.id),
        "origin": family.origin,
        "endpoint_epoch": family.endpoint_epoch,
        "scope": family.scope,
        "name": device.name,
        "fingerprint": fingerprint(device.identity_public_key_b64),
        "issuer": family.issuer,
    }


async def renew(
    db, user_id, client_binding, device_id, family_id, endpoint_epoch, issuer, now
):
    await _client_active(db, user_id, client_binding, now)
    device = await current_device(db, device_id)
    family = await _family(db, family_id)
    endpoint = await _endpoint(db, device_id)
    if family is None or (family.user_id, family.client_binding, family.device_id) != (
        user_id,
        client_binding,
        device_id,
    ):
        refuse()
    if (
        canonical_origin(issuer) != family.issuer
        or endpoint_epoch != family.endpoint_epoch
        or not _valid_family(family, endpoint, device)
        or not await _parent_active(db, family, now)
    ):
        refuse()
    return await _mint(db, family, device, now)


async def _family(db, family_id):
    try:
        return await db.get(BridgeHttpFamily, UUID(str(family_id)))
    except ValueError:
        return None


def _valid_family(family, endpoint, device):
    if not _valid_endpoint(endpoint, device) or family.revoked_at is not None:
        return False
    return (
        family.user_id,
        family.origin,
        family.endpoint_epoch,
        family.authorization_epoch,
    ) == (
        device.owner_user_id,
        endpoint.origin,
        endpoint.endpoint_epoch,
        device.authorization_epoch,
    )


async def introspect(db, raw, assertion, issuer, now):
    device = await current_device(db, proof_device_id(assertion))
    claims = verify(assertion, device, "introspection", issuer, now)
    if not isinstance(raw, str) or not raw.startswith("bhttp_") or len(raw) > 128:
        return {"active": False}
    if claims.get("token_hash") != hash_token(raw):
        refuse()
    await _record_replay(db, device, claims, now)
    token = await db.scalar(
        select(BridgeHttpToken).where(BridgeHttpToken.token_hash == hash_token(raw))
    )
    if token is None or token.expires_at <= now:
        return {"active": False}
    family = await db.get(BridgeHttpFamily, token.family_id)
    endpoint = await _endpoint(db, device.id)
    if (
        family.issuer != canonical_origin(issuer)
        or family.device_id != device.id
        or not _valid_family(family, endpoint, device)
        or not await _parent_active(db, family, now)
    ):
        return {"active": False}
    return {
        "active": True,
        "issuer": canonical_origin(issuer),
        "account_id": str(family.user_id),
        "device_id": str(device.id),
        "family_id": str(family.id),
        "origin": family.origin,
        "endpoint_epoch": family.endpoint_epoch,
        "authorization_epoch": family.authorization_epoch,
        "scope": family.scope,
        "exp": int(token.expires_at.timestamp()),
        "token_id": str(token.id),
    }


async def revoke_client(db, user_id, client_binding, now):
    await db.scalar(select(User).where(User.id == user_id).with_for_update())
    binding = await _client(db, user_id, client_binding)
    if binding is None:
        binding = BridgeHttpClient(
            user_id=user_id, binding_hash=hash_token(client_binding)
        )
        db.add(binding)
    binding.revoked_at = now
    await db.execute(
        update(BridgeHttpChallenge)
        .where(
            BridgeHttpChallenge.user_id == user_id,
            BridgeHttpChallenge.client_binding == client_binding,
        )
        .values(consumed_at=now)
    )
    await db.execute(
        update(BridgeHttpFamily)
        .where(
            BridgeHttpFamily.user_id == user_id,
            BridgeHttpFamily.client_binding == client_binding,
        )
        .values(revoked_at=now)
    )


async def revoke_family(db, user_id, client_binding, device_id, family_id, now):
    family = await _family(db, family_id)
    if family is None or (family.user_id, family.client_binding, family.device_id) != (
        user_id,
        client_binding,
        device_id,
    ):
        refuse()
    family.revoked_at = now


async def revoke_device(db, device_id, now, *, bump_epoch=True):
    if bump_epoch:
        await db.execute(
            update(Device)
            .where(Device.id == device_id)
            .values(authorization_epoch=Device.authorization_epoch + 1)
        )
    await db.execute(
        update(BridgeHttpEndpoint)
        .where(BridgeHttpEndpoint.device_id == device_id)
        .values(enabled=False)
    )
    await db.execute(
        update(BridgeHttpFamily)
        .where(BridgeHttpFamily.device_id == device_id)
        .values(revoked_at=now)
    )


async def _client(db, user_id, client_binding):
    return await db.scalar(
        select(BridgeHttpClient).where(
            BridgeHttpClient.user_id == user_id,
            BridgeHttpClient.binding_hash == hash_token(client_binding),
        )
    )


async def _client_active(db, user_id, client_binding, now):
    if not client_binding.startswith(("browser:", "desktop:")):
        refuse()
    # Same row lock as logout: no discovery can race a logout tombstone.
    await db.scalar(select(User).where(User.id == user_id).with_for_update())
    binding = await _client(db, user_id, client_binding)
    if binding is not None and binding.revoked_at is not None:
        refuse()
    # Only the authentication-completion hook creates active browser identities.
    # Older/lazy cookie values must never become a new authenticated login here.
    if client_binding.startswith("browser:") and binding is None:
        refuse()
    parent = _parent_fields(client_binding)
    if parent:
        if parent["parent_expires_at"] <= now or await oauth2_service.is_token_revoked(
            db, parent["parent_jti"]
        ):
            refuse()


def _parent_fields(client_binding):
    if not client_binding.startswith("desktop:"):
        return {}
    try:
        _, jti, exp = client_binding.split(":")
        if not 1 <= len(jti) <= 64:
            refuse()
        return {
            "parent_jti": jti,
            "parent_expires_at": datetime.fromtimestamp(int(exp), timezone.utc),
        }
    except (ValueError, OverflowError):
        refuse()


async def _parent_active(db, family, now):
    if family.client_binding.startswith("browser:"):
        binding = await _client(db, family.user_id, family.client_binding)
        return binding is not None and binding.revoked_at is None
    if not family.client_binding.startswith("desktop:") or family.parent_expires_at is None:
        return False
    return family.parent_expires_at > now and not await oauth2_service.is_token_revoked(
        db, family.parent_jti
    )


async def _active_member(db, user_id):
    active = await db.scalar(select(User.is_active).where(User.id == user_id))
    return bool(active) and await is_alpha_member(db, user_id)
