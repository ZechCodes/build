"""Issuer policy and transport discovery for an approved pairing."""

import os
from uuid import UUID

from litestar.exceptions import PermissionDeniedException
from sqlalchemy import select, update
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import hosted_tunnel_source
from buildapp.accounts import AccountPolicyValues, account_policy
from buildapp.bridge_http_auth import endpoint_is_current, endpoint_matches_pairing
from buildapp.bridge_http_models import BridgeHttpEndpoint
from buildapp.models import Device


RTC_BOOTSTRAP_ENV = "BUILD_RTC_BOOTSTRAP"


async def mark_hosted_ready_once(db_session: AsyncSession, pairing: Device) -> bool:
    """Latch first readiness for this pairing in the caller's transaction."""
    if not pairing.approved or pairing.owner_user_id is None:
        return False
    # A revoked row can be purged and later recreated with the same device ID.
    # The creation stamp fences that case, even if the new row restarts at epoch 0.
    updated = await db_session.scalar(
        update(Device)
        .where(
            Device.id == pairing.id,
            Device.owner_user_id == pairing.owner_user_id,
            Device.approved.is_(True),
            Device.authorization_epoch == pairing.authorization_epoch,
            Device.created_at == pairing.created_at,
            Device.hosted_ready_once.is_(False),
        )
        .values(hosted_ready_once=True)
        .returning(Device.id)
    )
    return updated is not None


async def rtc_allowed(
    db_session: AsyncSession, device: Device, *, policy: AccountPolicyValues | None = None,
) -> bool:
    """Decide RTC eligibility for a pairing using its owner's current policy."""
    owner = device.owner_user_id
    if owner is None:
        return True
    if policy is None:
        policy = await account_policy(db_session, owner)
    if not policy.tunnel_access:
        return True
    if device.hosted_ready_once or os.environ.get(RTC_BOOTSTRAP_ENV, "on") != "on":
        return False
    endpoint = await db_session.scalar(
        select(BridgeHttpEndpoint).where(BridgeHttpEndpoint.device_id == device.id)
    )
    # A disabled personal listener still has a configured address.
    return (
        endpoint is None or endpoint.assignment_generation is not None
        or not endpoint_matches_pairing(endpoint, device)
    )


async def require_rtc_mint(
    db_session: AsyncSession, user_id: UUID, target: str | None,
) -> None:
    """Require a device eligible for RTC when the caller has tunnel access."""
    if not (await account_policy(db_session, user_id)).tunnel_access:
        return
    try:
        device_id = UUID(target or "")
    except ValueError as exc:
        raise PermissionDeniedException("RTC requires an eligible device") from exc
    device = await db_session.get(Device, device_id)
    if (
        device is None or not device.approved or device.owner_user_id != user_id
        or not await rtc_allowed(db_session, device)
    ):
        raise PermissionDeniedException("RTC requires an eligible device")


async def transport_projection(
    db_session: AsyncSession, device: Device, endpoint: BridgeHttpEndpoint | None, *, policy: AccountPolicyValues,
) -> dict:
    current = endpoint if await endpoint_is_current(db_session, endpoint, device) else None
    source, origin, reason, revision = "webrtc", None, None, None
    if current is not None:
        origin, revision = current.origin, current.assignment_generation
        source = "hosted" if revision is not None else "personal"
    else:
        assignment = await hosted_tunnel_source.current_assignment(db_session, device)
        if assignment is not None:
            source, revision = "hosted", assignment.generation
            if assignment.state == "pending":
                reason = hosted_tunnel_source.setup_reason(assignment)
    return {
        "source": source,
        "rtc_allowed": await rtc_allowed(db_session, device, policy=policy),
        "reason": reason,
        "origin": origin,
        "assignment_revision": revision,
    }
