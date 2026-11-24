"""Signed hosted-mode reports and one-use connector credential delivery."""

import base64
import hmac
import json
import re
from uuid import uuid4

from sqlalchemy import select

from buildapp import bridge_http_auth as auth
from buildapp import hosted_tunnel_source as source
from buildapp.bridge_http_models import BridgeHttpChallenge
from buildapp.models import HostedModeReport

CHALLENGE = "hosted-tunnel-challenge"
CLAIM = "hosted-tunnel-claim"


async def _saved_report(db, device):
    return await db.scalar(
        select(HostedModeReport)
        .where(HostedModeReport.device_id == device.id)
        .execution_options(populate_existing=True)
    )


def _read_report(claims):
    mode, capable = claims.get("mode"), claims.get("hosted_capable")
    if mode not in ("off", "personal", "hosted") or type(capable) is not bool:
        auth.refuse()
    if re.fullmatch(r"[A-Za-z0-9_-]{16,128}", claims["jti"]) is None:
        auth.refuse()
    return mode, capable


def _pairing_fields(device):
    return device.owner_user_id, device.created_at, device.authorization_epoch


def _report_fields(row):
    return row.owner_user_id, row.pairing_created_at, row.authorization_epoch


def _binding(device, saved, report, generation):
    # Store the digest in the generic challenge binding column. Its purpose
    # distinguishes it from the browser session binding on discovery rows.
    payload = (*_pairing_fields(device), saved.revision if saved else None, *report, generation)
    return auth.hash_token(json.dumps(payload, default=str))


async def issue_challenge(db, device_id, proof, issuer, now):
    device = await auth.current_device(db, device_id)
    claims = auth.verify(proof, device, CHALLENGE, issuer, now)
    report = _read_report(claims)
    async with db.begin_nested():
        await auth._record_replay(db, device, claims, now)
        scope = (BridgeHttpChallenge.device_id == device.id) & (BridgeHttpChallenge.purpose == CLAIM)
        await auth._bounded_challenges(db, scope, now, 8)
        saved = await _saved_report(db, device)
        row = await source.current_assignment(db, device) if report == ("hosted", True) else None
        generation = row.generation if source.ready_assignment(row) else None
        challenge, nonce = await auth._challenge(
            db, now, purpose=CLAIM, device_id=device.id, user_id=device.owner_user_id,
            client_binding=_binding(device, saved, report, generation),
        )
    return {
        "challenge_id": str(challenge.id),
        "nonce": nonce,
        "expires_at": int(challenge.expires_at.timestamp()),
        "generation": generation,
    }


def _check_challenge(challenge, device, saved, report, claims):
    generation = claims.get("generation")
    if "generation" not in claims or (
        generation is not None and not source.valid_generation(generation)
    ):
        auth.refuse()
    if (challenge.device_id, challenge.user_id) != (device.id, device.owner_user_id):
        auth.refuse()
    if not hmac.compare_digest(challenge.client_binding or "", _binding(device, saved, report, generation)):
        auth.refuse()
    nonce = claims.get("nonce")
    if not isinstance(nonce, str) or re.fullmatch(r"[A-Za-z0-9_-]{43}", nonce) is None:
        auth.refuse()


async def claim_assignment(db, device_id, proof, issuer, now):
    device = await auth.current_device(db, device_id)
    claims = auth.verify(proof, device, CLAIM, issuer, now)
    report = _read_report(claims)
    challenge = await auth._get_challenge(db, claims.get("challenge_id"), CLAIM)
    await auth._record_replay(db, device, claims, now)
    saved = await _saved_report(db, device)
    _check_challenge(challenge, device, saved, report, claims)
    await auth._spend(db, challenge, claims["nonce"], now)
    await _save_report(db, device, saved, report)
    return await _claim_result(db, device, report, claims["generation"], now)


async def _save_report(db, device, saved, report):
    pairing = _pairing_fields(device)
    if saved is not None and (
        _report_fields(saved), (saved.mode, saved.hosted_capable),
    ) == (pairing, report):
        return
    if saved is None:
        saved = HostedModeReport(device_id=device.id)
        db.add(saved)
    saved.owner_user_id, saved.pairing_created_at, saved.authorization_epoch = pairing
    saved.mode, saved.hosted_capable = report
    saved.revision = uuid4()
    await db.flush()
    await source.notify_hosted_tunnels_changed(db)


async def _claim_result(db, device, report, generation, now):
    mode, _ = report
    entitled = await source.has_tunnel_access(db, device)
    if not entitled or report != ("hosted", True):
        endpoint = await auth._endpoint(db, device.id)
        if endpoint is not None and endpoint.assignment_generation is not None:
            await auth.withdraw_hosted_registration(
                db, device.id, endpoint.origin, endpoint.assignment_generation, now,
            )
        if not entitled or mode == "off":
            return {"status": "off"}
        if mode == "personal":
            return {"status": "personal"}
        return _setup("update_required")
    return await _release_current(db, device, generation)


def _setup(reason):
    return {"status": "setup", "reason": reason}


def _assignment_version(row):
    # Snapshot immutable values before another SQL read refreshes the ORM object.
    return (row.hostname, row.generation, row.tunnel_id, row.dns_record_id, row.encrypted_secret)


async def _release_current(db, device, generation):
    row = await source.current_assignment(db, device)
    if not source.ready_assignment(row):
        if generation is not None:
            return _setup("assignment_changed")
        return _setup(source.setup_reason(row))
    if generation != row.generation:
        return _setup("assignment_changed")
    version = _assignment_version(row)
    credentials = _checked_credentials(source.materialize_credentials(row), row.tunnel_id)
    if not await auth._active_member(db, device.owner_user_id):
        auth.refuse()
    latest = await source.current_assignment(db, device)
    if not source.ready_assignment(latest) or version != _assignment_version(latest):
        return _setup("assignment_changed")
    return {
        "status": "assigned", "hostname": latest.hostname, "generation": latest.generation,
        "credentials": credentials,
    }


def _checked_credentials(credentials, tunnel_id):
    if set(credentials) != {"AccountTag", "TunnelID", "TunnelSecret"}:
        raise ValueError("Invalid connector credential")
    account, tunnel, secret = (credentials[key] for key in ("AccountTag", "TunnelID", "TunnelSecret"))
    if (
        not isinstance(account, str) or re.fullmatch(r"[a-zA-Z0-9_-]{1,128}", account) is None
        or tunnel != str(tunnel_id) or not isinstance(secret, str) or len(secret) > 256
        or len(base64.b64decode(secret, validate=True)) < 32
    ):
        raise ValueError("Invalid connector credential")
    return credentials
